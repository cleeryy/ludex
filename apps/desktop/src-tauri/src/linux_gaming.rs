//! Launch planning for Windows games on Linux (#89) and per-game Wine
//! prefixes (#92).
//!
//! The interesting parts are pure functions: tool selection, the environment a
//! UMU/Proton/Wine launch needs, prefix path derivation, environment and
//! winetricks validation. [`crate::launch`] and [`crate::installer`] only apply
//! the result, so the fallbacks are unit tested without Steam or Proton.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::compat::{CompatTool, ToolKind};

/// How a game is started on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LaunchMode {
    /// Native binaries run directly; Windows executables go through the best
    /// installed tool (UMU, then Proton, then Wine).
    #[default]
    Auto,
    /// Run the file directly, no compatibility layer.
    Native,
    /// Use a Proton installation (`proton run game.exe`).
    Proton,
    /// Use `umu-run` with a Proton build (or UMU's own Proton).
    Umu,
}

impl LaunchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LaunchMode::Auto => "auto",
            LaunchMode::Native => "native",
            LaunchMode::Proton => "proton",
            LaunchMode::Umu => "umu",
        }
    }
}

/// Per-game launch options, stored in the desktop index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchOptions {
    pub mode: LaunchMode,
    /// Preferred tool id from `compat_scan`; empty means "best available".
    pub tool_id: String,
    /// Custom Wine prefix; empty means `{installDir}/.prefix`.
    pub prefix_path: String,
    /// Extra environment variables (`DXVK_*`, `PROTON_*`, `WINEDLLOVERRIDES`…).
    pub env: BTreeMap<String, String>,
    /// Winetricks verbs run on demand in the game prefix.
    pub winetricks_verbs: Vec<String>,
    /// Steam app id: ProtonDB rating, `GAMEID` and the Steam match.
    pub steam_app_id: Option<u32>,
    /// Folder the game process starts in (#187). Empty keeps the automatic
    /// rule: adopted games run from the executable's folder, installs from the
    /// install root. Stored as an absolute path, or relative to the install
    /// folder when the user typed a relative one.
    pub working_dir: String,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            mode: LaunchMode::Auto,
            tool_id: String::new(),
            prefix_path: String::new(),
            env: BTreeMap::new(),
            winetricks_verbs: Vec::new(),
            steam_app_id: None,
            working_dir: String::new(),
        }
    }
}

impl LaunchOptions {
    /// Trim every user value; validation is separate so errors are explicit.
    pub fn normalize(&mut self) {
        self.tool_id = self.tool_id.trim().to_string();
        self.prefix_path = self.prefix_path.trim().to_string();
        self.working_dir = self.working_dir.trim().to_string();
        self.env = self
            .env
            .iter()
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .filter(|(key, _)| !key.is_empty())
            .collect();
        self.winetricks_verbs = self
            .winetricks_verbs
            .iter()
            .map(|verb| verb.trim().to_ascii_lowercase())
            .filter(|verb| !verb.is_empty())
            .collect();
    }
}

/// Environment keys refused because they would break the tool itself.
const RESERVED_ENV: [&str; 5] = [
    "PATH",
    "HOME",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "PYTHONPATH",
];
const MAX_ENV_VARS: usize = 64;
const MAX_VERBS: usize = 32;

/// Validate and normalise user environment overrides.
pub fn validate_env(env: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    if env.len() > MAX_ENV_VARS {
        return Err(format!(
            "Too many environment variables ({}, at most {MAX_ENV_VARS}).",
            env.len()
        ));
    }
    let mut cleaned = BTreeMap::new();
    for (key, value) in env {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let mut chars = key.chars();
        let valid_start = chars
            .next()
            .map(|character| character.is_ascii_alphabetic() || character == '_')
            .unwrap_or(false);
        let valid_rest =
            chars.all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid_start || !valid_rest || key.len() > 64 {
            return Err(format!(
                "Environment variable names must match [A-Za-z_][A-Za-z0-9_]* (got {key:?})."
            ));
        }
        if RESERVED_ENV.contains(&key) {
            return Err(format!("{key} cannot be overridden per game."));
        }
        if value.contains('\0') {
            return Err(format!("The value of {key} contains a NUL byte."));
        }
        if value.len() > 4096 {
            return Err(format!(
                "The value of {key} is too long (4096 characters max)."
            ));
        }
        cleaned.insert(key.to_string(), value.clone());
    }
    Ok(cleaned)
}

/// Validate winetricks verbs: lowercase identifiers, no leading flag.
pub fn validate_verbs(verbs: &[String]) -> Result<Vec<String>, String> {
    let mut cleaned: Vec<String> = Vec::new();
    for verb in verbs {
        let verb = verb.trim().to_ascii_lowercase();
        if verb.is_empty() {
            continue;
        }
        if verb.len() > 64 {
            return Err(format!("The winetricks verb {verb:?} is too long."));
        }
        let mut chars = verb.chars();
        let valid_start = chars
            .next()
            .map(|character| character.is_ascii_alphanumeric())
            .unwrap_or(false);
        let valid_rest = chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '+')
        });
        if !valid_start || !valid_rest {
            return Err(format!(
                "Invalid winetricks verb {verb:?}: use letters, digits, '-', '_', '.' or '+'."
            ));
        }
        if !cleaned.contains(&verb) {
            cleaned.push(verb);
        }
    }
    if cleaned.len() > MAX_VERBS {
        return Err(format!(
            "At most {MAX_VERBS} winetricks verbs can be stored."
        ));
    }
    Ok(cleaned)
}

/// Verbs offered by the UI; any other valid verb can still be typed.
pub fn suggested_verbs() -> Vec<String> {
    [
        "corefonts",
        "vcrun2019",
        "dotnet48",
        "d3dx9",
        "xact",
        "faudio",
        "mfplat",
        "dxvk",
        "vkd3d",
    ]
    .iter()
    .map(|verb| (*verb).to_string())
    .collect()
}

/// Wine prefix for a game: the custom path, or `{installPath}/.prefix`.
pub fn prefix_for(install_path: &Path, custom: &str) -> Result<PathBuf, String> {
    let custom = custom.trim();
    if custom.is_empty() {
        return Ok(install_path.join(".prefix"));
    }
    let path = PathBuf::from(custom);
    if !path.is_absolute() {
        return Err("The Wine prefix must be an absolute path.".to_string());
    }
    if path == Path::new("/") {
        return Err("The Wine prefix cannot be the filesystem root.".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("The Wine prefix must not contain '..'.".to_string());
    }
    Ok(path)
}

/// A tool resolved for a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatSelection {
    pub tool_id: String,
    pub name: String,
    pub kind: ToolKind,
    /// Executable: `umu-run`, the `proton` script or `wine`.
    pub program: PathBuf,
    /// Proton directory passed as `PROTONPATH`.
    pub proton_path: Option<PathBuf>,
    /// Steam root used as `STEAM_COMPAT_CLIENT_INSTALL_PATH`.
    pub client_install_path: Option<PathBuf>,
}

/// Pick a tool. `preferred` is a scan id; when it is gone the next best tool is
/// used instead of failing the launch.
pub fn select_tool(
    tools: &[CompatTool],
    mode: LaunchMode,
    preferred: &str,
) -> Result<CompatSelection, String> {
    if mode == LaunchMode::Native {
        return Err("Native launch mode does not use a compatibility tool.".to_string());
    }

    let allowed = |kind: ToolKind| match mode {
        LaunchMode::Umu => kind == ToolKind::Umu,
        LaunchMode::Proton => kind == ToolKind::Proton,
        _ => true,
    };

    let preferred = preferred.trim();
    let chosen = if preferred.is_empty() {
        None
    } else {
        tools
            .iter()
            .find(|tool| tool.id == preferred && allowed(tool.kind))
    };

    let chosen = chosen.or_else(|| match mode {
        LaunchMode::Umu => tools.iter().find(|tool| tool.kind == ToolKind::Umu),
        LaunchMode::Proton => tools.iter().find(|tool| tool.kind == ToolKind::Proton),
        // Auto: UMU, then Proton (GE first), then Wine.
        _ => [ToolKind::Umu, ToolKind::Proton, ToolKind::Wine]
            .iter()
            .find_map(|kind| tools.iter().find(|tool| tool.kind == *kind)),
    });

    let tool = chosen.ok_or_else(|| match mode {
        LaunchMode::Umu => {
            "umu-run is not installed. Install umu-launcher, or point Settings → Compatibility at it."
                .to_string()
        }
        LaunchMode::Proton => {
            "No Proton installation was found. Install GE-Proton/Proton in Steam, or set a Proton folder in Settings → Compatibility."
                .to_string()
        }
        _ => "No compatibility tool is installed. Install UMU (umu-launcher), GE-Proton/Proton or Wine, then rescan in Settings → Compatibility."
            .to_string(),
    })?;

    // UMU can fetch its own Proton, but a locally installed build is faster and
    // pins the version the user already has. Prefer it when one is visible.
    let proton_path = match tool.kind {
        ToolKind::Umu => tool.proton_path.clone().or_else(|| {
            tools
                .iter()
                .find(|candidate| candidate.kind == ToolKind::Proton)
                .and_then(|candidate| candidate.proton_path.clone())
        }),
        _ => tool.proton_path.clone(),
    };

    Ok(CompatSelection {
        tool_id: tool.id.clone(),
        name: tool.name.clone(),
        kind: tool.kind,
        program: PathBuf::from(&tool.path),
        proton_path: proton_path.map(PathBuf::from),
        client_install_path: None,
    })
}

/// A fully built launch, ready to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub prefix: PathBuf,
    pub tool_id: String,
    pub tool_name: String,
}

impl LaunchPlan {
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        self.env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

/// Build the command and environment for one launch.
///
/// - UMU: `umu-run <exe>` with `PROTONPATH`, `GAMEID`, `STEAM_COMPAT_DATA_PATH`,
///   `STEAM_COMPAT_CLIENT_INSTALL_PATH`, `WINEPREFIX` and `UMU_LOG`.
/// - Proton: `<proton> run <exe>` with the Steam compatibility variables; the
///   Wine prefix Proton creates is `<prefix>/pfx`.
/// - Wine: `wine <exe>` with `WINEPREFIX=<prefix>`.
///
/// Per-game overrides are applied last and can replace any of these values.
pub fn build_launch(
    selection: &CompatSelection,
    executable: &Path,
    extra_args: &[String],
    prefix: &Path,
    options: &LaunchOptions,
) -> Result<LaunchPlan, String> {
    let prefix_text = prefix.to_string_lossy().to_string();
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let mut args: Vec<String> = Vec::new();

    match selection.kind {
        ToolKind::Umu => {
            args.push(executable.to_string_lossy().to_string());
            args.extend(extra_args.iter().cloned());
            env.insert("STEAM_COMPAT_DATA_PATH".into(), prefix_text.clone());
            env.insert("WINEPREFIX".into(), prefix_text.clone());
            env.insert(
                "PROTONPATH".into(),
                selection
                    .proton_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string())
                    .unwrap_or_else(|| "GE-Proton".to_string()),
            );
            env.insert(
                "GAMEID".into(),
                options
                    .steam_app_id
                    .map(|app_id| app_id.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            );
            env.insert("UMU_LOG".into(), "warn".into());
            if let Some(client) = &selection.client_install_path {
                env.insert(
                    "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                    client.to_string_lossy().to_string(),
                );
            }
        }
        ToolKind::Proton => {
            args.push("run".into());
            args.push(executable.to_string_lossy().to_string());
            args.extend(extra_args.iter().cloned());
            env.insert("STEAM_COMPAT_DATA_PATH".into(), prefix_text.clone());
            env.insert(
                "WINEPREFIX".into(),
                prefix.join("pfx").to_string_lossy().to_string(),
            );
            if let Some(client) = &selection.client_install_path {
                env.insert(
                    "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                    client.to_string_lossy().to_string(),
                );
            }
        }
        ToolKind::Wine => {
            args.push(executable.to_string_lossy().to_string());
            args.extend(extra_args.iter().cloned());
            env.insert("WINEPREFIX".into(), prefix_text.clone());
        }
    }

    let overrides = validate_env(&options.env)?;
    for (key, value) in overrides {
        env.insert(key, value);
    }

    Ok(LaunchPlan {
        program: selection.program.clone(),
        args,
        env,
        prefix: prefix.to_path_buf(),
        tool_id: selection.tool_id.clone(),
        tool_name: selection.name.clone(),
    })
}

/// A winetricks invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinetricksCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// `winetricks -q <verbs>` against the game prefix, using the Proton Wine
/// binary when the selected tool is a Proton build.
pub fn build_winetricks(
    winetricks: &Path,
    verbs: &[String],
    prefix: &Path,
    selection: Option<&CompatSelection>,
) -> Result<WinetricksCommand, String> {
    let verbs = validate_verbs(verbs)?;
    if verbs.is_empty() {
        return Err("Choose at least one winetricks verb.".to_string());
    }

    let mut args = vec!["-q".to_string()];
    args.extend(verbs);

    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("WINEPREFIX".into(), prefix.to_string_lossy().to_string());

    if let Some(selection) = selection {
        match selection.kind {
            ToolKind::Proton | ToolKind::Umu => {
                env.insert(
                    "STEAM_COMPAT_DATA_PATH".into(),
                    prefix.to_string_lossy().to_string(),
                );
                if let Some(client) = &selection.client_install_path {
                    env.insert(
                        "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                        client.to_string_lossy().to_string(),
                    );
                }
                if let Some(proton) = &selection.proton_path {
                    // Winetricks needs the Proton Wine build, not the system one.
                    let wine = proton.join("files/bin/wine");
                    if wine.is_file() {
                        env.insert("WINE".into(), wine.to_string_lossy().to_string());
                    }
                    let wineserver = proton.join("files/bin/wineserver");
                    if wineserver.is_file() {
                        env.insert(
                            "WINESERVER".into(),
                            wineserver.to_string_lossy().to_string(),
                        );
                    }
                }
            }
            ToolKind::Wine => {}
        }
    }

    Ok(WinetricksCommand {
        program: winetricks.to_path_buf(),
        args,
        env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(kind: ToolKind, name: &str, path: &str, proton: Option<&str>) -> CompatTool {
        CompatTool {
            id: format!("{}:{path}", kind.as_str()),
            name: name.to_string(),
            kind,
            version: None,
            path: path.to_string(),
            proton_path: proton.map(str::to_string),
            source: "path".into(),
            steam_root: None,
        }
    }

    fn tools() -> Vec<CompatTool> {
        vec![
            tool(ToolKind::Umu, "umu-run", "/usr/bin/umu-run", None),
            tool(
                ToolKind::Proton,
                "GE-Proton9-20",
                "/games/GE-Proton9-20/proton",
                Some("/games/GE-Proton9-20"),
            ),
            tool(ToolKind::Wine, "wine", "/usr/bin/wine", None),
        ]
    }

    fn prefix() -> PathBuf {
        PathBuf::from("/games/hades/.prefix")
    }

    fn exe() -> PathBuf {
        PathBuf::from("/games/hades/Hades.exe")
    }

    #[test]
    fn auto_prefers_umu_then_proton_then_wine() {
        let all = tools();
        assert_eq!(
            select_tool(&all, LaunchMode::Auto, "").unwrap().kind,
            ToolKind::Umu
        );
        assert_eq!(
            select_tool(&all[1..], LaunchMode::Auto, "").unwrap().kind,
            ToolKind::Proton
        );
        assert_eq!(
            select_tool(&all[2..], LaunchMode::Auto, "").unwrap().kind,
            ToolKind::Wine
        );
    }

    #[test]
    fn explicit_modes_pick_their_kind_and_preferred_ids_win() {
        let all = tools();
        assert_eq!(
            select_tool(&all, LaunchMode::Proton, "").unwrap().name,
            "GE-Proton9-20"
        );
        let wine = select_tool(&all, LaunchMode::Auto, &all[2].id).unwrap();
        assert_eq!(wine.kind, ToolKind::Wine);
        // A preferred tool that no longer exists falls back instead of failing.
        let fallback = select_tool(&all, LaunchMode::Auto, &all[0].id).unwrap();
        assert_eq!(fallback.tool_id, all[0].id);
        let gone = select_tool(&all, LaunchMode::Auto, "proton:/gone").unwrap();
        assert_eq!(gone.kind, ToolKind::Umu);
    }

    #[test]
    fn missing_tools_produce_actionable_errors() {
        let empty: Vec<CompatTool> = Vec::new();
        let error = select_tool(&empty, LaunchMode::Auto, "").unwrap_err();
        assert!(error.contains("UMU") && error.contains("Wine"));
        assert!(select_tool(&empty, LaunchMode::Proton, "")
            .unwrap_err()
            .contains("Proton"));
        assert!(select_tool(&tools(), LaunchMode::Native, "").is_err());
    }

    #[test]
    fn umu_environment_carries_proton_gameid_and_compat_paths() {
        let all = tools();
        let selection = select_tool(&all, LaunchMode::Umu, "").unwrap();
        let mut selection = selection;
        selection.client_install_path = Some(PathBuf::from("/home/player/.local/share/Steam"));
        let options = LaunchOptions {
            steam_app_id: Some(1145360),
            ..LaunchOptions::default()
        };

        let plan = build_launch(
            &selection,
            &exe(),
            &["--windowed".to_string()],
            &prefix(),
            &options,
        )
        .unwrap();
        assert_eq!(plan.program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(
            plan.args,
            vec![
                "/games/hades/Hades.exe".to_string(),
                "--windowed".to_string()
            ]
        );
        assert_eq!(plan.env["STEAM_COMPAT_DATA_PATH"], "/games/hades/.prefix");
        assert_eq!(plan.env["WINEPREFIX"], "/games/hades/.prefix");
        assert_eq!(plan.env["PROTONPATH"], "/games/GE-Proton9-20");
        assert_eq!(plan.env["GAMEID"], "1145360");
        assert_eq!(plan.env["UMU_LOG"], "warn");
        assert_eq!(
            plan.env["STEAM_COMPAT_CLIENT_INSTALL_PATH"],
            "/home/player/.local/share/Steam"
        );
        assert_eq!(plan.tool_id, all[0].id);
    }

    #[test]
    fn umu_without_a_proton_build_asks_umu_for_ge_proton() {
        let mut selection = select_tool(&tools(), LaunchMode::Umu, "").unwrap();
        selection.proton_path = None;
        let plan = build_launch(
            &selection,
            &exe(),
            &[],
            &prefix(),
            &LaunchOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.env["PROTONPATH"], "GE-Proton");
        assert_eq!(plan.env["GAMEID"], "0");
    }

    #[test]
    fn proton_runs_through_the_proton_script_with_pfx_prefix() {
        let selection = select_tool(&tools(), LaunchMode::Proton, "").unwrap();
        let plan = build_launch(
            &selection,
            &exe(),
            &[],
            &prefix(),
            &LaunchOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.program, PathBuf::from("/games/GE-Proton9-20/proton"));
        assert_eq!(
            plan.args,
            vec!["run".to_string(), "/games/hades/Hades.exe".to_string()]
        );
        assert_eq!(plan.env["STEAM_COMPAT_DATA_PATH"], "/games/hades/.prefix");
        assert_eq!(plan.env["WINEPREFIX"], "/games/hades/.prefix/pfx");
        assert!(!plan.env.contains_key("GAMEID"));
    }

    #[test]
    fn wine_uses_wineprefix_and_overrides_are_applied_last() {
        let selection = select_tool(&tools(), LaunchMode::Auto, &tools()[2].id).unwrap();
        let mut options = LaunchOptions::default();
        options.env.insert("DXVK_HUD".into(), "fps".into());
        options.env.insert("PROTON_NO_ESYNC".into(), "1".into());
        let plan = build_launch(&selection, &exe(), &[], &prefix(), &options).unwrap();
        assert_eq!(plan.program, PathBuf::from("/usr/bin/wine"));
        assert_eq!(plan.env["WINEPREFIX"], "/games/hades/.prefix");
        assert_eq!(plan.env["DXVK_HUD"], "fps");
        assert_eq!(plan.env["PROTON_NO_ESYNC"], "1");
    }

    #[test]
    fn environment_validation_rejects_dangerous_or_malformed_keys() {
        let mut env = BTreeMap::new();
        env.insert("1BAD".to_string(), "x".into());
        assert!(validate_env(&env).is_err());

        env.clear();
        env.insert("LD_PRELOAD".to_string(), "/evil.so".into());
        assert!(validate_env(&env).unwrap_err().contains("LD_PRELOAD"));

        env.clear();
        env.insert("DXVK_STATE_CACHE".to_string(), "1".into());
        assert_eq!(validate_env(&env).unwrap().len(), 1);
    }

    #[test]
    fn verb_validation_normalises_and_rejects_flags() {
        let verbs = validate_verbs(&[
            " VCrun2019 ".into(),
            "vcrun2019".into(),
            "d3dx9".into(),
            String::new(),
        ])
        .unwrap();
        assert_eq!(verbs, vec!["vcrun2019".to_string(), "d3dx9".to_string()]);

        assert!(validate_verbs(&["-q".into()]).is_err());
        assert!(validate_verbs(&["bad verb".into()]).is_err());
        assert!(validate_verbs(&["ok;rm".into()]).is_err());
        assert!(validate_verbs(&[]).unwrap().is_empty());
    }

    #[test]
    fn prefix_paths_default_inside_the_install_folder() {
        let install = Path::new("/games/hades/default");
        assert_eq!(
            prefix_for(install, "").unwrap(),
            PathBuf::from("/games/hades/default/.prefix")
        );
        assert_eq!(
            prefix_for(install, " /mnt/prefixes/hades ").unwrap(),
            PathBuf::from("/mnt/prefixes/hades")
        );
        assert!(prefix_for(install, "relative/prefix").is_err());
        assert!(prefix_for(install, "/").is_err());
        assert!(prefix_for(install, "/games/../etc").is_err());
    }

    #[test]
    fn winetricks_builder_uses_the_proton_wine_binary_when_present() {
        let root = std::env::temp_dir().join(format!(
            "ludex-winetricks-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let proton = root.join("GE-Proton9-20");
        std::fs::create_dir_all(proton.join("files/bin")).unwrap();
        std::fs::write(proton.join("files/bin/wine"), b"x").unwrap();
        std::fs::write(proton.join("proton"), b"x").unwrap();

        let tool = tool(
            ToolKind::Proton,
            "GE-Proton9-20",
            &proton.join("proton").to_string_lossy(),
            Some(&proton.to_string_lossy()),
        );
        let selection = select_tool(&[tool], LaunchMode::Proton, "").unwrap();
        let command = build_winetricks(
            Path::new("/usr/bin/winetricks"),
            &["vcrun2019".into(), "corefonts".into()],
            &prefix(),
            Some(&selection),
        )
        .unwrap();

        assert_eq!(
            command.args,
            vec![
                "-q".to_string(),
                "vcrun2019".to_string(),
                "corefonts".to_string()
            ]
        );
        assert_eq!(command.env["WINEPREFIX"], "/games/hades/.prefix");
        assert!(command.env["WINE"].ends_with("files/bin/wine"));
        assert!(!command.env.contains_key("WINESERVER"));
        assert!(build_winetricks(Path::new("/usr/bin/winetricks"), &[], &prefix(), None).is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
