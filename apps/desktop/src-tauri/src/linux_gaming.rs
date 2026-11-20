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
use crate::display_caps::{Capability, DisplayCapabilities};

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
    /// Run a Proton build. `select_tool` routes it through UMU when `umu-run`
    /// is installed (#279); the bare `proton run` script is only the fallback
    /// for a machine without UMU.
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
    /// Per-game display profile through Gamescope (#287). Default is the
    /// desktop, untouched.
    pub display: DisplayProfile,
    /// Audio output sink for this game (#289). Empty follows the profile
    /// default: the global override, then the sound server's own default.
    pub audio_sink: String,
    /// In-memory only: the launch plan asked for GameMode (#297).
    ///
    /// Set by `launch_plan::apply_to_options`, read by the Linux launch path to
    /// wrap the entry point with `gamemoderun`. Never written to the database
    /// and never part of the recovery profile hash, so a saved or reverted
    /// profile keeps the user's own decisions only.
    #[serde(skip)]
    pub gamemode: bool,
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
            display: DisplayProfile::default(),
            audio_sink: String::new(),
            gamemode: false,
        }
    }
}

impl LaunchOptions {
    /// Trim every user value; validation is separate so errors are explicit.
    pub fn normalize(&mut self) {
        self.tool_id = self.tool_id.trim().to_string();
        self.prefix_path = self.prefix_path.trim().to_string();
        self.working_dir = self.working_dir.trim().to_string();
        self.audio_sink = self.audio_sink.trim().to_string();
        self.display.filter = self.display.filter.trim().to_ascii_lowercase();
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

/// Where a Linux game is presented (#287).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// Launch into the running desktop session and leave its mode alone. This
    /// is the default and the only option without Gamescope.
    #[default]
    Desktop,
    /// Run the game inside a nested Gamescope session.
    Gamescope,
}

/// Per-game display profile (#287).
///
/// The default is a strict no-op: desktop mode, no dimensions, no refresh and
/// no flags. A game without a profile launches exactly like a build that never
/// had this feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayProfile {
    pub mode: DisplayMode,
    /// Game width in pixels. `0` until a Gamescope profile is enabled.
    pub width: u32,
    /// Game height in pixels.
    pub height: u32,
    /// Game refresh rate in Hz; `0` leaves it to Gamescope.
    pub refresh: u32,
    /// Gamescope upscaler filter (`linear`, `nearest`, `fsr`, `nis`, `pixel`,
    /// `sgsr`); empty keeps the Gamescope default.
    pub filter: String,
    /// Ask Gamescope for adaptive sync (#292). Applied only when the installed
    /// build advertises `--adaptive-sync` and the active output supports VRR.
    pub adaptive_sync: bool,
    /// Ask Gamescope for HDR output. Applied only when the installed build
    /// advertises `--hdr-enabled` and the output advertises HDR metadata.
    pub hdr: bool,
}

impl DisplayProfile {
    pub fn is_gamescope(&self) -> bool {
        self.mode == DisplayMode::Gamescope
    }

    /// `Some((width, height))` for a profile that can be rendered, `None`
    /// otherwise.
    pub fn resolution(&self) -> Option<(u32, u32)> {
        let valid = DISPLAY_WIDTH_RANGE.contains(&self.width)
            && DISPLAY_HEIGHT_RANGE.contains(&self.height);
        valid.then_some((self.width, self.height))
    }
}

/// Bounds validated at save time and re-checked before a launch. A stored
/// profile outside them falls back to the desktop instead of failing the run.
pub const DISPLAY_WIDTH_RANGE: std::ops::RangeInclusive<u32> = 320..=7680;
pub const DISPLAY_HEIGHT_RANGE: std::ops::RangeInclusive<u32> = 200..=4320;
pub const DISPLAY_MAX_REFRESH: u32 = 480;
/// Filters documented by `gamescope --help` (`--filter`), newest last.
pub const DISPLAY_FILTERS: [&str; 6] = ["linear", "nearest", "fsr", "nis", "pixel", "sgsr"];

/// Validate and normalise a display profile.
///
/// `desktop` accepts anything: the profile is inert and old values survive a
/// toggle. A Gamescope profile needs a real resolution; the refresh rate stays
/// optional (`0` = Gamescope default) and the filter must be a documented one.
pub fn validate_display(profile: &mut DisplayProfile) -> Result<(), String> {
    profile.filter = profile.filter.trim().to_ascii_lowercase();
    if !profile.is_gamescope() {
        return Ok(());
    }
    if !DISPLAY_WIDTH_RANGE.contains(&profile.width) {
        return Err(format!(
            "The Gamescope width must be between {} and {} pixels.",
            DISPLAY_WIDTH_RANGE.start(),
            DISPLAY_WIDTH_RANGE.end()
        ));
    }
    if !DISPLAY_HEIGHT_RANGE.contains(&profile.height) {
        return Err(format!(
            "The Gamescope height must be between {} and {} pixels.",
            DISPLAY_HEIGHT_RANGE.start(),
            DISPLAY_HEIGHT_RANGE.end()
        ));
    }
    if profile.refresh > DISPLAY_MAX_REFRESH {
        return Err(format!(
            "The Gamescope refresh rate must be 0 (desktop) or at most {DISPLAY_MAX_REFRESH} Hz."
        ));
    }
    if !profile.filter.is_empty() && !DISPLAY_FILTERS.contains(&profile.filter.as_str()) {
        return Err(format!(
            "Unknown Gamescope filter {:?}; use one of {}.",
            profile.filter,
            DISPLAY_FILTERS.join(", ")
        ));
    }
    Ok(())
}

/// What an installed Gamescope advertises (#287).
///
/// Detection runs `gamescope --help` once per scan: that command prints the
/// version and the option table before any display is opened, so it is safe on
/// a headless host. Nothing here is guessed from a version number; a flag that
/// is absent from the help output is treated as unsupported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GamescopeInfo {
    /// Absolute `gamescope` executable path.
    pub path: String,
    /// Version printed by `gamescope --help` / `--version`.
    pub version: Option<String>,
    /// `--hdr-enabled` is in the help output.
    pub hdr: bool,
    /// `--adaptive-sync` is in the help output (#292).
    pub adaptive_sync: bool,
    /// Upscaler filters the help output lists for `--filter`. Empty means the
    /// flag is unsupported (or its value list could not be parsed).
    pub filters: Vec<String>,
}

impl GamescopeInfo {
    pub fn program(&self) -> PathBuf {
        PathBuf::from(&self.path)
    }

    pub fn supports_filter(&self, filter: &str) -> bool {
        self.filters.iter().any(|value| value == filter)
    }
}

/// Parse `gamescope --help` output (stdout + stderr) without running anything.
///
/// The usage text looks like:
///
/// ```text
/// gamescope version 3.15.14
/// usage: gamescope [options...] -- [command...]
///   -w, --nested-width             game width
///   -h, --nested-height            game height
///   -r, --nested-refresh           game refresh rate (frames per second)
///   -F, --filter                   upscaler filter (linear, nearest, fsr, nis, pixel, sgsr)
///   --hdr-enabled                  enable HDR output ...
///   --adaptive-sync                enable adaptive sync ...
/// ```
pub fn parse_gamescope_help(help: &str) -> GamescopeInfo {
    let version = help.lines().find_map(|line| {
        let index = line.to_ascii_lowercase().find(" version ")?;
        let value = line[index + " version ".len()..]
            .split_whitespace()
            .next()
            .unwrap_or_default();
        (!value.is_empty()).then(|| value.to_string())
    });

    let filters = help
        .lines()
        .find(|line| line.contains("--filter"))
        .and_then(|line| {
            let open = line.find('(')?;
            let close = line[open..].find(')')? + open;
            Some(
                line[open + 1..close]
                    .split(',')
                    .map(str::trim)
                    .filter(|value| {
                        !value.is_empty()
                            && value
                                .chars()
                                .all(|character| character.is_ascii_alphanumeric())
                    })
                    .map(|value| value.to_ascii_lowercase())
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();

    GamescopeInfo {
        path: String::new(),
        version,
        hdr: help.contains("--hdr-enabled"),
        adaptive_sync: help.contains("--adaptive-sync"),
        filters,
    }
}

/// Probe an installed Gamescope executable. A probe that cannot run still
/// reports the binary, just without any supported flag.
pub fn probe_gamescope(program: &Path) -> GamescopeInfo {
    // #181: an internal probe, never a window; `capture` also hides the
    // console on Windows for a stray `gamescope.exe`.
    let help = crate::process::capture(program)
        .arg("--help")
        .output()
        .map(|output| {
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .unwrap_or_default();
    let mut info = parse_gamescope_help(&help);
    info.path = program.to_string_lossy().to_string();
    info
}

/// The gamescope flags and dropped requests for one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GamescopeCommand {
    /// `--hdr-enabled` made it into the command.
    pub hdr: bool,
    /// `--adaptive-sync` made it into the command (#292).
    pub adaptive_sync: bool,
    /// Filter actually applied, when the installed build offers it.
    pub filter: Option<String>,
    /// Requests the build could not honour; logged, never fatal.
    pub warnings: Vec<String>,
}

/// What [`resolve_display_with_caps`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayLaunch {
    /// Run the resolved program and arguments as-is.
    Desktop,
    /// Prefix them with `gamescope`.
    Gamescope(GamescopeCommand),
    /// A profile was requested but cannot be honoured; the caller runs the
    /// unwrapped fallback and explains why.
    Fallback { reason: String },
}

/// The command to spawn after the display decision plus the decision itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLaunch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub display: DisplayLaunch,
}

impl ResolvedLaunch {
    pub fn gamescope(&self) -> Option<&GamescopeCommand> {
        match &self.display {
            DisplayLaunch::Gamescope(command) => Some(command),
            _ => None,
        }
    }

    pub fn fallback_reason(&self) -> Option<&str> {
        match &self.display {
            DisplayLaunch::Fallback { reason } => Some(reason),
            _ => None,
        }
    }
}

/// Decide how to present one launch with per-output capabilities (#292).
///
/// The only flags used are the nested-mode ones from `gamescope --help`:
/// `-w/--nested-width`, `-h/--nested-height`, `-r/--nested-refresh`,
/// `--adaptive-sync`, `--hdr-enabled` and `--filter`, separated from the game
/// by `--`.
///
/// Deliberately absent: `-f/--fullscreen` (a fullscreen nested window would
/// cover the desktop), `--backend drm` (an exclusive session), and
/// `-O/--prefer-output` (a DRM connector selector). Gamescope therefore stays
/// nested and the desktop mode is never changed.
///
/// Adaptive sync and HDR are dropped with a warning when the build does not
/// advertise the flag, or when the active output reports the capability as
/// incompatible. An unreadable capability (`unknown`) never blocks a flag: only
/// an explicit "incapable" is a reason to fall back.
///
/// Every failure path returns the unwrapped command: a missing binary, an
/// incomplete profile, a flag this build does not advertise.
pub fn resolve_display_with_caps(
    profile: &DisplayProfile,
    gamescope: Option<&GamescopeInfo>,
    capabilities: Option<&DisplayCapabilities>,
    program: &Path,
    args: &[String],
) -> ResolvedLaunch {
    let unwrapped = |fallback: Option<String>| ResolvedLaunch {
        program: program.to_path_buf(),
        args: args.to_vec(),
        display: match fallback {
            Some(reason) => DisplayLaunch::Fallback { reason },
            None => DisplayLaunch::Desktop,
        },
    };

    if !profile.is_gamescope() {
        return unwrapped(None);
    }
    let Some(gamescope) = gamescope else {
        return unwrapped(Some(
            "Gamescope is not installed on this machine; the game launches into the desktop."
                .to_string(),
        ));
    };
    let Some((width, height)) = profile.resolution() else {
        return unwrapped(Some(format!(
            "The Gamescope profile has no usable resolution ({}x{}); the game launches into the desktop.",
            profile.width, profile.height
        )));
    };

    let mut warnings = Vec::new();
    let mut gamescope_args = vec![
        "-w".to_string(),
        width.to_string(),
        "-h".to_string(),
        height.to_string(),
    ];

    if profile.refresh > 0 {
        if profile.refresh <= DISPLAY_MAX_REFRESH {
            gamescope_args.push("-r".to_string());
            gamescope_args.push(profile.refresh.to_string());
        } else {
            warnings.push(format!(
                "The {} Hz refresh rate is above the {DISPLAY_MAX_REFRESH} Hz limit; Gamescope keeps the desktop rate.",
                profile.refresh
            ));
        }
    }

    let adaptive_sync = synchronisation_requested(profile, gamescope, capabilities, &mut warnings);
    if adaptive_sync {
        gamescope_args.push("--adaptive-sync".to_string());
    }

    let hdr = hdr_requested(profile, gamescope, capabilities, &mut warnings);
    if hdr {
        gamescope_args.push("--hdr-enabled".to_string());
    }

    let filter = profile.filter.trim().to_ascii_lowercase();
    let filter = if filter.is_empty() {
        None
    } else if gamescope.supports_filter(&filter) {
        Some(filter)
    } else {
        warnings.push(format!(
            "This Gamescope build does not offer the {filter} filter; it was not applied."
        ));
        None
    };
    if let Some(filter) = &filter {
        gamescope_args.push("--filter".to_string());
        gamescope_args.push(filter.clone());
    }

    gamescope_args.push("--".to_string());
    gamescope_args.push(program.to_string_lossy().to_string());
    gamescope_args.extend(args.iter().cloned());

    ResolvedLaunch {
        program: gamescope.program(),
        args: gamescope_args,
        display: DisplayLaunch::Gamescope(GamescopeCommand {
            hdr,
            adaptive_sync,
            filter,
            warnings,
        }),
    }
}

/// Whether `--adaptive-sync` reaches the command (#292).
fn synchronisation_requested(
    profile: &DisplayProfile,
    gamescope: &GamescopeInfo,
    capabilities: Option<&DisplayCapabilities>,
    warnings: &mut Vec<String>,
) -> bool {
    if !profile.adaptive_sync {
        return false;
    }
    if !gamescope.adaptive_sync {
        warnings.push(
            "This Gamescope build does not advertise --adaptive-sync; adaptive sync was not requested."
                .to_string(),
        );
        return false;
    }
    if capabilities.map(DisplayCapabilities::vrr) == Some(Capability::Incapable) {
        warnings.push(
            "This output reports VRR as incompatible; adaptive sync was not requested.".to_string(),
        );
        return false;
    }
    true
}

/// Whether `--hdr-enabled` reaches the command (#292).
fn hdr_requested(
    profile: &DisplayProfile,
    gamescope: &GamescopeInfo,
    capabilities: Option<&DisplayCapabilities>,
    warnings: &mut Vec<String>,
) -> bool {
    if !profile.hdr {
        return false;
    }
    if !gamescope.hdr {
        warnings.push(
            "This Gamescope build does not advertise --hdr-enabled; HDR was not requested."
                .to_string(),
        );
        return false;
    }
    if capabilities.map(DisplayCapabilities::hdr) == Some(Capability::Incapable) {
        warnings.push("The output has no HDR metadata; HDR was not requested.".to_string());
        return false;
    }
    true
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

/// Variables the AppImage runtime (`AppRun`) exports for the bundle itself.
///
/// They point inside the transient `/.mount_*` directory and must not reach a
/// game's compatibility tool (#278):
///
/// - `PYTHONHOME`/`PYTHONPATH`/`PYTHONNOUSERSITE`: `umu-run` is a Python
///   zipapp. With the bundle's stdlib path it dies on `Failed to import
///   encodings` before any Proton code runs.
/// - `LD_LIBRARY_PATH`/`LD_PRELOAD`: the bundle ships an older `libcrypto`, so
///   the host Python loads the mount's OpenSSL 3.2 and fails on `OPENSSL_3.3.0`
///   symbols. A preload also changes what every Wine binary links against.
/// - `GTK_*`, `GDK_PIXBUF_MODULE_FILE`, `GSETTINGS_SCHEMA_DIR`,
///   `GIO_*`: theme/loader caches of the bundle; Wine and Proton dialogs would
///   load the wrong gtk modules and schema files.
pub const APPIMAGE_BUNDLE_ENV: [&str; 12] = [
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONNOUSERSITE",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "GTK_PATH",
    "GTK_EXE_PREFIX",
    "GTK_DATA_PREFIX",
    "GDK_PIXBUF_MODULE_FILE",
    "GSETTINGS_SCHEMA_DIR",
    "GIO_EXTRA_MODULES",
    "GIO_MODULE_DIR",
];

/// The child-environment changes an AppImage launch needs (#278).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppImageEnv {
    /// Variables removed from the child environment. Removing an unset
    /// variable is a no-op, so the list is always complete.
    pub removed: Vec<&'static str>,
    /// Replacement `PATH`; `None` when no entry pointed into the mount.
    pub path: Option<String>,
    /// How many `PATH` entries were dropped because they lived in `APPDIR`.
    pub dropped_path_entries: usize,
}

/// Whether `path` is `root` or inside it, compared component by component so a
/// trailing slash or a `..`-free variant still matches.
fn path_is_under(path: &Path, root: &Path) -> bool {
    if root.as_os_str().is_empty() {
        return false;
    }
    let mut path = path.components();
    root.components()
        .all(|root_part| path.next() == Some(root_part))
}

/// Read the real process environment; see [`appimage_env_from`].
pub fn appimage_env() -> Option<AppImageEnv> {
    appimage_env_from(|key| std::env::var(key).ok())
}

/// Environment cleanup for a launch started from an AppImage (#278).
///
/// Returns `None` for a host install: a distro/Steam install keeps its own
/// environment untouched. Inside an AppImage the compatibility tool is a
/// *host* program (`umu-run`, the Proton scripts, `wine`), so it must not
/// inherit the bundle's Python, loader or Gtk paths, and `PATH` must not keep
/// the mount's `usr/bin`, which would shadow the host `python3`/`ldd`.
///
/// `lookup` is `std::env::var` in production and a map in tests.
pub fn appimage_env_from(lookup: impl Fn(&str) -> Option<String>) -> Option<AppImageEnv> {
    let value = |key: &str| {
        lookup(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };
    if value("APPIMAGE").is_none() && value("APPDIR").is_none() {
        return None;
    }

    let mut env = AppImageEnv {
        removed: APPIMAGE_BUNDLE_ENV.to_vec(),
        path: None,
        dropped_path_entries: 0,
    };

    // Without APPDIR there is no mount to filter; the removals still apply.
    let Some(appdir) = value("APPDIR") else {
        return Some(env);
    };
    let Some(path) = lookup("PATH") else {
        return Some(env);
    };
    let root = Path::new(&appdir);
    let mut kept: Vec<&str> = Vec::new();
    for entry in path.split(':') {
        if !entry.is_empty() && path_is_under(Path::new(entry), root) {
            env.dropped_path_entries += 1;
            continue;
        }
        kept.push(entry);
    }
    // An AppImage mount is always prepended to the host PATH, so something
    // remains in practice; never hand a child an empty PATH (it breaks every
    // shell and dlopen lookup) if the mount was all there was.
    if env.dropped_path_entries > 0 && !kept.is_empty() {
        env.path = Some(kept.join(":"));
    }

    Some(env)
}

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
///
/// #279: `LaunchMode::Proton` still picks the user's Proton build, but the
/// launch goes through UMU when `umu-run` is installed. Running the `proton`
/// script bare only works for Steam's own compatdata layout; UMU sets up the
/// Steam Linux Runtime, the prefix layout and the emulator variables. Bare
/// Proton is therefore only the fallback when UMU is absent.
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

    let mut selection = CompatSelection {
        tool_id: tool.id.clone(),
        name: tool.name.clone(),
        kind: tool.kind,
        program: PathBuf::from(&tool.path),
        proton_path: proton_path.map(PathBuf::from),
        client_install_path: None,
    };

    // #279: keep the chosen Proton build (id, name and PROTONPATH) but run it
    // through UMU. Only the missing UMU makes the bare script the fallback.
    if mode == LaunchMode::Proton {
        if let Some(umu) = tools
            .iter()
            .find(|candidate| candidate.kind == ToolKind::Umu)
        {
            // The scan always fills `proton_path`; the script's folder is the
            // same directory for hand-made tools and fixtures.
            let proton_dir = selection
                .proton_path
                .clone()
                .or_else(|| selection.program.parent().map(Path::to_path_buf));
            selection.kind = ToolKind::Umu;
            selection.program = PathBuf::from(&umu.path);
            selection.proton_path = proton_dir;
        }
    }

    Ok(selection)
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
///   Wine prefix Proton creates is `<prefix>/pfx`. Fallback only: `select_tool`
///   hands this kind to UMU whenever `umu-run` is installed (#279).
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
        let proton = select_tool(&all, LaunchMode::Proton, "").unwrap();
        assert_eq!(proton.name, "GE-Proton9-20");
        assert_eq!(proton.tool_id, all[1].id);
        let wine = select_tool(&all, LaunchMode::Auto, &all[2].id).unwrap();
        assert_eq!(wine.kind, ToolKind::Wine);
        // A preferred tool that no longer exists falls back instead of failing.
        let fallback = select_tool(&all, LaunchMode::Auto, &all[0].id).unwrap();
        assert_eq!(fallback.tool_id, all[0].id);
        let gone = select_tool(&all, LaunchMode::Auto, "proton:/gone").unwrap();
        assert_eq!(gone.kind, ToolKind::Umu);
    }

    /// #279: choosing a Proton build on a machine with UMU runs that build as
    /// `PROTONPATH` through `umu-run`, while the UI keeps the Proton name.
    #[test]
    fn proton_mode_routes_the_chosen_build_through_umu() {
        let all = tools();
        let selection = select_tool(&all, LaunchMode::Proton, "").unwrap();
        assert_eq!(selection.kind, ToolKind::Umu);
        assert_eq!(selection.program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(selection.tool_id, all[1].id);
        assert_eq!(selection.name, "GE-Proton9-20");
        assert_eq!(
            selection.proton_path,
            Some(PathBuf::from("/games/GE-Proton9-20"))
        );

        let plan = build_launch(
            &selection,
            &exe(),
            &[],
            &prefix(),
            &LaunchOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(plan.args, vec!["/games/hades/Hades.exe".to_string()]);
        assert_eq!(plan.env["PROTONPATH"], "/games/GE-Proton9-20");
        // UMU owns the prefix directly; no `pfx` subfolder.
        assert_eq!(plan.env["WINEPREFIX"], "/games/hades/.prefix");
        assert_eq!(plan.tool_name, "GE-Proton9-20");

        // `umu` mode itself is unchanged: no Proton preference is invented.
        let umu = select_tool(&all, LaunchMode::Umu, "").unwrap();
        assert_eq!(umu.kind, ToolKind::Umu);
        assert_eq!(umu.program, PathBuf::from("/usr/bin/umu-run"));
    }

    #[test]
    fn proton_mode_keeps_a_non_default_preferred_build_under_umu() {
        let all = vec![
            tool(ToolKind::Umu, "umu-run", "/usr/bin/umu-run", None),
            tool(
                ToolKind::Proton,
                "GE-Proton9-20",
                "/games/GE-Proton9-20/proton",
                Some("/games/GE-Proton9-20"),
            ),
            tool(
                ToolKind::Proton,
                "Proton - Experimental",
                "/steam/Proton - Experimental/proton",
                Some("/steam/Proton - Experimental"),
            ),
        ];
        let selection = select_tool(&all, LaunchMode::Proton, &all[2].id).unwrap();
        assert_eq!(selection.kind, ToolKind::Umu);
        assert_eq!(selection.tool_id, all[2].id);
        assert_eq!(selection.name, "Proton - Experimental");
        assert_eq!(
            selection.proton_path,
            Some(PathBuf::from("/steam/Proton - Experimental"))
        );

        let plan = build_launch(
            &selection,
            &exe(),
            &[],
            &prefix(),
            &LaunchOptions::default(),
        )
        .unwrap();
        assert_eq!(plan.env["PROTONPATH"], "/steam/Proton - Experimental");
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
    fn proton_without_umu_falls_back_to_the_bare_proton_script() {
        // #279: the direct `proton run` command is only for machines with no
        // UMU at all.
        let protons: Vec<CompatTool> = tools()
            .into_iter()
            .filter(|tool| tool.kind != ToolKind::Umu)
            .collect();
        let selection = select_tool(&protons, LaunchMode::Proton, "").unwrap();
        assert_eq!(selection.kind, ToolKind::Proton);
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

        // #278: the AppImage cleanup does not open these up to per-game values.
        env.clear();
        env.insert("PYTHONPATH".to_string(), "/tmp".into());
        assert!(validate_env(&env).unwrap_err().contains("PYTHONPATH"));

        env.clear();
        env.insert("DXVK_STATE_CACHE".to_string(), "1".into());
        assert_eq!(validate_env(&env).unwrap().len(), 1);
    }

    fn appimage_lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    /// #278: a host install never touches the child environment; an AppImage
    /// removes the bundle variables and drops the mount from `PATH`.
    #[test]
    fn appimage_environment_is_cleaned_only_inside_an_appimage() {
        assert_eq!(
            appimage_env_from(appimage_lookup(&[("PATH", "/usr/bin:/bin")])),
            None
        );

        let cleanup = appimage_env_from(appimage_lookup(&[
            ("APPIMAGE", "/home/player/Apps/Ludex.AppImage"),
            ("APPDIR", "/tmp/.mount_Ludex_keDGlJ"),
            (
                "PATH",
                "/tmp/.mount_Ludex_keDGlJ/usr/bin/:/usr/local/bin:/usr/bin:/bin",
            ),
            ("PYTHONHOME", "/tmp/.mount_Ludex_keDGlJ/usr/"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_Ludex_keDGlJ/usr/lib"),
        ]))
        .unwrap();
        assert_eq!(cleanup.removed, APPIMAGE_BUNDLE_ENV.to_vec());
        assert!(cleanup.removed.contains(&"PYTHONNOUSERSITE"));
        assert!(cleanup.removed.contains(&"GIO_EXTRA_MODULES"));
        assert_eq!(
            cleanup.path.as_deref(),
            Some("/usr/local/bin:/usr/bin:/bin")
        );
        assert_eq!(cleanup.dropped_path_entries, 1);
    }

    #[test]
    fn appimage_path_entries_under_the_mount_are_dropped_wherever_they_appear() {
        let cleanup = appimage_env_from(appimage_lookup(&[
            ("APPDIR", "/tmp/.mount_Ludex_abc"),
            (
                "PATH",
                "/usr/local/bin:/tmp/.mount_Ludex_abc/usr/sbin/:/tmp/.mount_Ludex_abc:/opt/bin",
            ),
        ]))
        .unwrap();
        assert_eq!(cleanup.path.as_deref(), Some("/usr/local/bin:/opt/bin"));
        assert_eq!(cleanup.dropped_path_entries, 2);

        // A sibling mount or a lookalike prefix is kept: only the real mount
        // goes.
        let sibling = appimage_env_from(appimage_lookup(&[
            ("APPDIR", "/tmp/.mount_Ludex_abc"),
            ("PATH", "/tmp/.mount_Ludex_abc2/usr/bin:/usr/bin"),
        ]))
        .unwrap();
        assert_eq!(sibling.path, None);
        assert_eq!(sibling.dropped_path_entries, 0);
    }

    #[test]
    fn appimage_cleanup_is_idempotent_and_survives_a_missing_appdir() {
        let lookup = appimage_lookup(&[
            ("APPIMAGE", "/Apps/Ludex.AppImage"),
            ("APPDIR", "/tmp/.mount_Ludex_abc"),
        ]);
        let first = appimage_env_from(&lookup).unwrap();
        assert_eq!(
            first.removed,
            APPIMAGE_BUNDLE_ENV.to_vec(),
            "removals stay listed so env_remove is a no-op for unset values"
        );

        // A cleaned PATH produces no second rewrite.
        let second = appimage_env_from(appimage_lookup(&[
            ("APPIMAGE", "/Apps/Ludex.AppImage"),
            ("APPDIR", "/tmp/.mount_Ludex_abc"),
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
        ]))
        .unwrap();
        assert_eq!(second.path, None);
        assert_eq!(second.dropped_path_entries, 0);
        assert_eq!(second.removed.len(), 12);

        // `APPIMAGE` alone (no mount) still removes the bundle variables.
        let only_appimage =
            appimage_env_from(appimage_lookup(&[("APPIMAGE", "/Apps/Ludex.AppImage")])).unwrap();
        assert_eq!(only_appimage.path, None);
        assert_eq!(only_appimage.removed, APPIMAGE_BUNDLE_ENV.to_vec());
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

    /* --------------------------- Gamescope (#287) --------------------------- */

    /// The option table `gamescope --help` prints (verified against
    /// ValveSoftware/gamescope `src/main.cpp`, `usage[]`).
    const GAMESCOPE_HELP: &str = "\
gamescope version 3.15.14
usage: gamescope [options...] -- [command...]

Options:
  -W, --output-width             output width
  -H, --output-height            output height
  -w, --nested-width             game width
  -h, --nested-height            game height
  -r, --nested-refresh           game refresh rate (frames per second)
  -S, --scaler                   upscaler type (auto, integer, fit, fill, stretch)
  -F, --filter                   upscaler filter (linear, nearest, fsr, nis, pixel, sgsr)
  --backend                      select rendering backend
  --hdr-enabled                  enable HDR output (needs Gamescope WSI layer enabled for support from clients)
  --adaptive-sync                enable adaptive sync/VRR output
Nested mode options:
  -f, --fullscreen               make the window fullscreen
";

    fn gamescope() -> GamescopeInfo {
        GamescopeInfo {
            path: "/usr/bin/gamescope".to_string(),
            version: Some("3.15.14".to_string()),
            hdr: true,
            adaptive_sync: true,
            filters: vec![
                "linear".to_string(),
                "nearest".to_string(),
                "fsr".to_string(),
                "nis".to_string(),
                "pixel".to_string(),
                "sgsr".to_string(),
            ],
        }
    }

    fn game_profile() -> DisplayProfile {
        DisplayProfile {
            mode: DisplayMode::Gamescope,
            width: 1920,
            height: 1080,
            refresh: 60,
            filter: "fsr".to_string(),
            adaptive_sync: false,
            hdr: false,
        }
    }

    /// One connected output; `active` so `DisplayCapabilities::vrr()/hdr()`
    /// read this record.
    fn display_caps(vrr: Capability, hdr: Capability) -> DisplayCapabilities {
        DisplayCapabilities {
            outputs: vec![crate::display_caps::DisplayOutput {
                connector: "card0-DP-1".to_string(),
                status: "connected".to_string(),
                active: true,
                vrr,
                hdr,
                max_refresh: Some(240),
                color_depth: Some(10),
            }],
            active: Some("card0-DP-1".to_string()),
        }
    }

    fn native_args() -> Vec<String> {
        vec!["--windowed".to_string()]
    }

    /// #287 tests run without a capability probe; #292 tests pass the fixture.
    fn resolve(
        profile: &DisplayProfile,
        gamescope: Option<&GamescopeInfo>,
        program: &Path,
        args: &[String],
    ) -> ResolvedLaunch {
        resolve_display_with_caps(profile, gamescope, None, program, args)
    }

    #[test]
    fn gamescope_help_parsing_reports_version_hdr_adaptive_sync_and_filters() {
        let info = parse_gamescope_help(GAMESCOPE_HELP);
        assert_eq!(info.version.as_deref(), Some("3.15.14"));
        assert!(info.hdr);
        assert!(info.adaptive_sync);
        assert_eq!(
            info.filters,
            vec!["linear", "nearest", "fsr", "nis", "pixel", "sgsr"]
        );

        // An older build without HDR, adaptive sync or `--filter`: the parser
        // must not invent support from the version number.
        let old = parse_gamescope_help(
            "gamescope version 3.11.39\nusage: gamescope [options...] -- [command...]\n",
        );
        assert!(!old.hdr);
        assert!(!old.adaptive_sync);
        assert!(old.filters.is_empty());
        assert_eq!(old.version.as_deref(), Some("3.11.39"));
    }

    #[test]
    fn the_default_display_profile_is_a_no_op() {
        let options = LaunchOptions::default();
        assert_eq!(options.display.mode, DisplayMode::Desktop);
        assert_eq!(options.display, DisplayProfile::default());

        let resolved = resolve(&options.display, Some(&gamescope()), &exe(), &native_args());
        assert_eq!(resolved.display, DisplayLaunch::Desktop);
        assert_eq!(resolved.program, exe());
        assert_eq!(resolved.args, native_args());
        assert!(resolved.gamescope().is_none());
        assert!(resolved.fallback_reason().is_none());
    }

    #[test]
    fn a_gamescope_profile_wraps_a_native_command() {
        let profile = DisplayProfile {
            filter: "FSR".to_string(),
            ..game_profile()
        };
        let resolved = resolve(&profile, Some(&gamescope()), &exe(), &native_args());
        assert_eq!(resolved.program, PathBuf::from("/usr/bin/gamescope"));
        assert_eq!(
            resolved.args,
            vec![
                "-w".to_string(),
                "1920".to_string(),
                "-h".to_string(),
                "1080".to_string(),
                "-r".to_string(),
                "60".to_string(),
                "--filter".to_string(),
                "fsr".to_string(),
                "--".to_string(),
                "/games/hades/Hades.exe".to_string(),
                "--windowed".to_string(),
            ]
        );
        let command = resolved.gamescope().unwrap();
        assert!(!command.hdr);
        assert_eq!(command.filter.as_deref(), Some("fsr"));
        assert!(command.warnings.is_empty());

        // #287: nested only. No output size (DRM), no connector, no
        // fullscreen window and no exclusive backend: the desktop is untouched.
        for forbidden in ["-W", "-H", "-O", "-f", "--fullscreen", "--backend"] {
            assert!(
                !resolved.args.iter().any(|argument| argument == forbidden),
                "{forbidden} must never be passed to gamescope"
            );
        }
    }

    #[test]
    fn gamescope_wraps_umu_and_proton_via_umu() {
        let all = tools();
        let gamescope = gamescope();
        let profile = game_profile();

        let umu = select_tool(&all, LaunchMode::Umu, "").unwrap();
        let umu_plan =
            build_launch(&umu, &exe(), &[], &prefix(), &LaunchOptions::default()).unwrap();
        let resolved = resolve(
            &profile,
            Some(&gamescope),
            &umu_plan.program,
            &umu_plan.args,
        );
        assert_eq!(resolved.program, PathBuf::from("/usr/bin/gamescope"));
        assert_eq!(
            &resolved.args[resolved.args.len() - 2..],
            &[
                "/usr/bin/umu-run".to_string(),
                "/games/hades/Hades.exe".to_string()
            ]
        );

        // #279: Proton mode still picks the user's build but runs it through
        // UMU; the wrapper must sit outside that resolved command.
        let proton = select_tool(&all, LaunchMode::Proton, "").unwrap();
        let proton_plan =
            build_launch(&proton, &exe(), &[], &prefix(), &LaunchOptions::default()).unwrap();
        assert_eq!(proton_plan.program, PathBuf::from("/usr/bin/umu-run"));
        let resolved = resolve(
            &profile,
            Some(&gamescope),
            &proton_plan.program,
            &proton_plan.args,
        );
        assert_eq!(resolved.program, PathBuf::from("/usr/bin/gamescope"));
        assert!(resolved
            .args
            .iter()
            .any(|argument| argument == "/usr/bin/umu-run"));
        assert!(resolved
            .args
            .iter()
            .any(|argument| argument == "/games/hades/Hades.exe"));
    }

    #[test]
    fn gamescope_wraps_wine_and_the_bare_proton_script() {
        let profile = game_profile();

        let wine = select_tool(&tools(), LaunchMode::Auto, &tools()[2].id).unwrap();
        let wine_plan =
            build_launch(&wine, &exe(), &[], &prefix(), &LaunchOptions::default()).unwrap();
        let resolved = resolve(
            &profile,
            Some(&gamescope()),
            &wine_plan.program,
            &wine_plan.args,
        );
        assert_eq!(resolved.program, PathBuf::from("/usr/bin/gamescope"));
        assert_eq!(
            &resolved.args[resolved.args.len() - 2..],
            &[
                "/usr/bin/wine".to_string(),
                "/games/hades/Hades.exe".to_string()
            ]
        );

        let protons: Vec<CompatTool> = tools()
            .into_iter()
            .filter(|tool| tool.kind != ToolKind::Umu)
            .collect();
        let proton = select_tool(&protons, LaunchMode::Proton, "").unwrap();
        let proton_plan =
            build_launch(&proton, &exe(), &[], &prefix(), &LaunchOptions::default()).unwrap();
        let resolved = resolve(
            &profile,
            Some(&gamescope()),
            &proton_plan.program,
            &proton_plan.args,
        );
        assert_eq!(resolved.program, PathBuf::from("/usr/bin/gamescope"));
        // `proton run <exe>` stays one command behind the `--` separator.
        assert_eq!(
            &resolved.args[resolved.args.len() - 3..],
            &[
                "/games/GE-Proton9-20/proton".to_string(),
                "run".to_string(),
                "/games/hades/Hades.exe".to_string()
            ]
        );
    }

    #[test]
    fn a_missing_gamescope_falls_back_to_the_unwrapped_command() {
        let profile = game_profile();
        let resolved = resolve(&profile, None, &exe(), &native_args());
        assert_eq!(resolved.program, exe());
        assert_eq!(resolved.args, native_args());
        assert!(resolved.gamescope().is_none());
        assert!(resolved
            .fallback_reason()
            .unwrap()
            .contains("not installed"));
    }

    #[test]
    fn an_incomplete_profile_falls_back_instead_of_failing_the_launch() {
        let profile = DisplayProfile {
            width: 0,
            ..game_profile()
        };
        let resolved = resolve(&profile, Some(&gamescope()), &exe(), &native_args());
        assert_eq!(resolved.program, exe());
        assert_eq!(resolved.args, native_args());
        assert!(resolved.fallback_reason().unwrap().contains("resolution"));
    }

    #[test]
    fn hdr_is_requested_only_when_the_installed_build_advertises_it() {
        let profile = DisplayProfile {
            hdr: true,
            ..game_profile()
        };
        let supported = resolve(&profile, Some(&gamescope()), &exe(), &[]);
        let command = supported.gamescope().unwrap();
        assert!(command.hdr);
        assert!(supported.args.iter().any(|value| value == "--hdr-enabled"));
        assert!(command.warnings.is_empty());

        let mut older = gamescope();
        older.hdr = false;
        let unsupported = resolve(&profile, Some(&older), &exe(), &[]);
        let command = unsupported.gamescope().unwrap();
        assert!(!command.hdr);
        assert!(!unsupported
            .args
            .iter()
            .any(|value| value == "--hdr-enabled"));
        assert!(command
            .warnings
            .iter()
            .any(|warning| warning.contains("--hdr-enabled")));
    }

    #[test]
    fn adaptive_sync_is_requested_only_when_the_build_advertises_it() {
        let profile = DisplayProfile {
            adaptive_sync: true,
            ..game_profile()
        };
        let capable = display_caps(Capability::Supported, Capability::Supported);
        let supported =
            resolve_display_with_caps(&profile, Some(&gamescope()), Some(&capable), &exe(), &[]);
        let command = supported.gamescope().unwrap();
        assert!(command.adaptive_sync);
        assert!(!command.hdr);
        assert!(supported
            .args
            .iter()
            .any(|value| value == "--adaptive-sync"));
        assert!(command.warnings.is_empty());

        let mut older = gamescope();
        older.adaptive_sync = false;
        let unsupported =
            resolve_display_with_caps(&profile, Some(&older), Some(&capable), &exe(), &[]);
        let command = unsupported.gamescope().unwrap();
        assert!(!command.adaptive_sync);
        assert!(!unsupported
            .args
            .iter()
            .any(|value| value == "--adaptive-sync"));
        assert!(command
            .warnings
            .iter()
            .any(|warning| warning.contains("--adaptive-sync")));
    }

    #[test]
    fn adaptive_sync_falls_back_when_the_output_reports_vrr_incompatible() {
        let profile = DisplayProfile {
            adaptive_sync: true,
            ..game_profile()
        };
        let incapable = display_caps(Capability::Incapable, Capability::Supported);
        let resolved =
            resolve_display_with_caps(&profile, Some(&gamescope()), Some(&incapable), &exe(), &[]);
        let command = resolved.gamescope().unwrap();
        assert!(!command.adaptive_sync);
        assert!(!resolved.args.iter().any(|value| value == "--adaptive-sync"));
        assert!(command
            .warnings
            .iter()
            .any(|warning| warning.contains("VRR as incompatible")));

        // Unknown never blocks the flag: only an explicit "incapable" does.
        let unknown = display_caps(Capability::Unknown, Capability::Supported);
        let resolved =
            resolve_display_with_caps(&profile, Some(&gamescope()), Some(&unknown), &exe(), &[]);
        assert!(resolved.gamescope().unwrap().adaptive_sync);
    }

    #[test]
    fn hdr_falls_back_when_the_output_has_no_hdr_metadata() {
        let profile = DisplayProfile {
            hdr: true,
            ..game_profile()
        };
        let incapable = display_caps(Capability::Supported, Capability::Incapable);
        let resolved =
            resolve_display_with_caps(&profile, Some(&gamescope()), Some(&incapable), &exe(), &[]);
        let command = resolved.gamescope().unwrap();
        assert!(!command.hdr);
        assert!(!resolved.args.iter().any(|value| value == "--hdr-enabled"));
        assert!(command
            .warnings
            .iter()
            .any(|warning| warning.contains("HDR metadata")));

        let unknown = display_caps(Capability::Supported, Capability::Unknown);
        let resolved =
            resolve_display_with_caps(&profile, Some(&gamescope()), Some(&unknown), &exe(), &[]);
        assert!(resolved.gamescope().unwrap().hdr);
    }

    /// #292: a desktop launch never goes through Gamescope, so neither switch
    /// can change the desktop mode, the output routing or the colour pipeline.
    #[test]
    fn the_switches_never_touch_the_desktop() {
        let profile = DisplayProfile {
            adaptive_sync: true,
            hdr: true,
            ..DisplayProfile::default()
        };
        let capable = display_caps(Capability::Supported, Capability::Supported);
        let resolved = resolve_display_with_caps(
            &profile,
            Some(&gamescope()),
            Some(&capable),
            &exe(),
            &native_args(),
        );
        assert_eq!(resolved.display, DisplayLaunch::Desktop);
        assert_eq!(resolved.program, exe());
        assert_eq!(resolved.args, native_args());
        assert!(resolved.gamescope().is_none());

        // With a Gamescope profile the flags exist, but the wrapper still never
        // receives a connector, an exclusive backend or a fullscreen request.
        let profile = DisplayProfile {
            adaptive_sync: true,
            hdr: true,
            ..game_profile()
        };
        let resolved =
            resolve_display_with_caps(&profile, Some(&gamescope()), Some(&capable), &exe(), &[]);
        assert!(resolved.args.iter().any(|value| value == "--adaptive-sync"));
        assert!(resolved.args.iter().any(|value| value == "--hdr-enabled"));
        for forbidden in ["-W", "-H", "-O", "-f", "--fullscreen", "--backend"] {
            assert!(
                !resolved.args.iter().any(|argument| argument == forbidden),
                "{forbidden} must never be passed to gamescope"
            );
        }
    }

    #[test]
    fn a_filter_is_applied_only_when_the_build_lists_it() {
        let profile = DisplayProfile {
            filter: "fsr".to_string(),
            ..game_profile()
        };
        let mut older = gamescope();
        older.filters = vec!["linear".to_string(), "nearest".to_string()];
        let resolved = resolve(&profile, Some(&older), &exe(), &[]);
        let command = resolved.gamescope().unwrap();
        assert_eq!(command.filter, None);
        assert!(!resolved.args.iter().any(|value| value == "--filter"));
        assert!(command
            .warnings
            .iter()
            .any(|warning| warning.contains("fsr")));

        // No filter selected: no `--filter`, the build's default applies.
        let profile = DisplayProfile {
            filter: String::new(),
            ..game_profile()
        };
        let resolved = resolve(&profile, Some(&gamescope()), &exe(), &[]);
        assert_eq!(resolved.gamescope().unwrap().filter, None);
        assert!(!resolved.args.iter().any(|value| value == "--filter"));
    }

    #[test]
    fn a_zero_refresh_leaves_the_rate_to_gamescope() {
        let profile = DisplayProfile {
            refresh: 0,
            ..game_profile()
        };
        let resolved = resolve(&profile, Some(&gamescope()), &exe(), &[]);
        assert!(!resolved.args.iter().any(|value| value == "-r"));
    }

    #[test]
    fn display_validation_accepts_only_documented_values() {
        // The desktop default is inert and accepts old values.
        let mut profile = DisplayProfile {
            filter: " FSR ".to_string(),
            ..DisplayProfile::default()
        };
        validate_display(&mut profile).unwrap();
        assert_eq!(profile.filter, "fsr");

        let mut profile = game_profile();
        validate_display(&mut profile).unwrap();

        let mut too_wide = game_profile();
        too_wide.width = 0;
        assert!(validate_display(&mut too_wide).is_err());

        let mut too_fast = game_profile();
        too_fast.refresh = DISPLAY_MAX_REFRESH + 1;
        assert!(validate_display(&mut too_fast).is_err());

        let mut unknown = game_profile();
        unknown.filter = "bicubic".to_string();
        assert!(validate_display(&mut unknown).is_err());
    }

    #[test]
    fn a_per_game_profile_overrides_the_desktop_default() {
        // The global default (no profile) never wraps ...
        let default = LaunchOptions::default();
        assert!(resolve(&default.display, Some(&gamescope()), &exe(), &[])
            .gamescope()
            .is_none());

        // ... a per-game Gamescope profile does.
        let options = LaunchOptions {
            display: game_profile(),
            ..LaunchOptions::default()
        };
        assert!(resolve(&options.display, Some(&gamescope()), &exe(), &[])
            .gamescope()
            .is_some());
    }

    /// #287: the web type reads `display` in camelCase, and an older settings
    /// payload without the field must deserialise to the desktop default.
    #[test]
    fn the_display_profile_is_camel_case_and_defaults_to_desktop() {
        let options: LaunchOptions = serde_json::from_str(r#"{"mode":"auto"}"#).unwrap();
        assert_eq!(options.display, DisplayProfile::default());

        let options = LaunchOptions {
            display: game_profile(),
            ..LaunchOptions::default()
        };
        let json = serde_json::to_value(&options).unwrap();
        assert_eq!(json["display"]["mode"], "gamescope");
        assert_eq!(json["display"]["width"], 1920);
        assert_eq!(json["display"]["height"], 1080);
        assert_eq!(json["display"]["refresh"], 60);
        assert_eq!(json["display"]["filter"], "fsr");
        assert_eq!(json["display"]["adaptiveSync"], false);
        assert_eq!(json["display"]["hdr"], false);
    }
}
