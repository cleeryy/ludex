//! GOG-style portable Linux bundles (#332).
//!
//! An install folder becomes the game: `run.sh` and `install.sh` at the root, a
//! small `ludex-game.json` describing what the folder holds, and a `support/`
//! area for the Wine prefix and optional extra files. The folder can be moved,
//! renamed or copied to another Linux machine and started without Ludex.
//!
//! The scripts are generated from the same decisions the launcher uses
//! ([`crate::linux_gaming`]): the executable, the arguments, the environment
//! overrides and the runtime requirement. They never contain an absolute path:
//! not to Ludex, not to `$HOME`, not to the machine they were written on. Every
//! path is resolved from `SCRIPT_DIR` at run time.
//!
//! The portability contract is honest:
//!
//! * a **native** Linux build is self-contained as long as the target is the
//!   same architecture and has the system libraries it links against;
//! * a **Windows** build needs UMU, GE-Proton/Proton or Wine on the target. The
//!   scripts pick the best one that is installed and say exactly what to install
//!   when none is. A runtime bundled under `support/runtime` is honoured, but
//!   nothing is downloaded.
//!
//! `run.sh` refuses to start when `ludex-game.json` is missing or describes a
//! different game, so a stray script cannot be pointed at another folder.
//! Generated files always win over files an archive shipped in the same tree
//! (the replacement is logged).
//!
//! Generation is deliberately not part of the install transaction: a failure
//! leaves the install playable and only surfaces a status.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::linux_gaming::LaunchOptions;
use crate::paths;
use crate::state::AppState;

/// Version of the generated script set. `run.sh` refuses a manifest written by
/// another version instead of guessing how it differs.
pub const SCRIPT_VERSION: u32 = 1;

pub const RUN_SCRIPT: &str = "run.sh";
pub const INSTALL_SCRIPT: &str = "install.sh";
pub const MANIFEST_FILE: &str = "ludex-game.json";
pub const README_FILE: &str = "support/README.txt";
/// Prefix folder inside the bundle. Relative on purpose: the value travels with
/// the folder and is re-rooted by the scripts.
pub const PREFIX_DIR: &str = "support/prefix";
/// Optional checksum list; verified by `install.sh` when one is present.
pub const MANIFEST_SUMS: &str = "support/manifest.sha256";

/// Progress event of an export to a destination.
pub const PROGRESS_EVENT: &str = "portable://progress";
/// Status of the automatic generation that follows an install.
pub const STATUS_EVENT: &str = "portable://status";

const MAX_TITLE_CHARS: usize = 200;
const MAX_VERSION_CHARS: usize = 64;
const MAX_SLUG_CHARS: usize = 63;
const MAX_ARG_CHARS: usize = 1024;
const MAX_ARGS: usize = 64;
const MAX_PATH_CHARS: usize = 512;
/// Emit a copy progress event at most every this many bytes (plus always on the
/// last file).
const PROGRESS_STEP: u64 = 32 * 1024 * 1024;

/* ---------------------------------------------------------------------- */
/* The specification                                                       */
/* ---------------------------------------------------------------------- */

/// How the recorded executable starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableMode {
    /// A Linux binary: runs directly.
    Native,
    /// A Windows executable: needs a compatibility runtime.
    Windows,
}

impl PortableMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PortableMode::Native => "native",
            PortableMode::Windows => "windows",
        }
    }

    /// The extension decides, never the user's launch mode: a `.exe` cannot run
    /// on Linux without a compatibility layer even when the launch options say
    /// "native".
    pub fn detect(executable: &Path) -> Self {
        let windows = executable
            .extension()
            .map(|extension| extension.eq_ignore_ascii_case("exe"))
            .unwrap_or(false);
        if windows {
            PortableMode::Windows
        } else {
            PortableMode::Native
        }
    }
}

/// What the target machine must provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRequirement {
    /// Nothing to install: the binary runs as-is (same architecture and system
    /// libraries assumed).
    LinuxNative,
    /// UMU, GE-Proton/Proton or Wine (or a runtime in `support/runtime`).
    WindowsCompat,
}

impl RuntimeRequirement {
    pub fn as_str(self) -> &'static str {
        match self {
            RuntimeRequirement::LinuxNative => "linux-native",
            RuntimeRequirement::WindowsCompat => "windows-compat",
        }
    }

    pub fn for_mode(mode: PortableMode) -> Self {
        match mode {
            PortableMode::Native => RuntimeRequirement::LinuxNative,
            PortableMode::Windows => RuntimeRequirement::WindowsCompat,
        }
    }
}

/// Everything the generated files need. Paths are relative to the bundle root
/// and use `/`; nothing here is machine-specific except the timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableSpec {
    pub title: String,
    pub version: String,
    pub slug: String,
    /// Relative path of the game binary (`Hades.exe`, `bin/Game.sh`).
    pub executable: String,
    /// Relative folder the game starts in; `.` is the bundle root.
    pub working_dir: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub mode: PortableMode,
    /// `GAMEID` for UMU: the Steam app id, or `0` when there is none.
    pub gameid: String,
    /// Relative prefix folder (`support/prefix`).
    pub prefix: String,
    pub script_version: u32,
    /// RFC 3339 UTC of the generation.
    pub generated_at: String,
    /// `Ludex <version>`; no path, no host name.
    pub generator: String,
}

impl PortableSpec {
    /// Build the specification from a local install row and the per-game launch
    /// options. `args` is the launch-parameter string the game is started with
    /// (Ludex has no per-install argument store yet, so it defaults to empty).
    pub fn from_install(
        installed: &db::InstalledGame,
        options: &LaunchOptions,
        args: &str,
        now: &str,
        generator: &str,
    ) -> Result<Self, String> {
        let install_root = PathBuf::from(&installed.install_path);
        let recorded = installed
            .executable
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                "This install has no recorded executable yet. Pick the game binary in Ludex first."
                    .to_string()
            })?;
        let recorded = PathBuf::from(recorded);
        let absolute = if recorded.is_absolute() {
            paths::normalize_native_path(&recorded)
        } else {
            paths::normalize_native_path(&install_root.join(&recorded))
        };
        let root = paths::normalize_native_path(&install_root);
        let relative = absolute.strip_prefix(&root).map_err(|_| {
            "The recorded executable is outside the install folder; nothing portable can be built."
                .to_string()
        })?;
        let executable = relative_path(relative)?;
        if executable.is_empty() {
            return Err("The recorded executable is the install folder itself.".to_string());
        }

        // #187: an explicit working directory wins; it only ports when it lives
        // inside the folder, otherwise the bundle root is used.
        let working_dir = {
            let raw = options.working_dir.trim();
            if raw.is_empty() {
                ".".to_string()
            } else {
                let candidate = PathBuf::from(raw);
                let full = if candidate.is_absolute() {
                    candidate
                } else {
                    root.join(candidate)
                };
                let full = paths::normalize_native_path(&full);
                match full
                    .strip_prefix(&root)
                    .ok()
                    .and_then(|rel| relative_path(rel).ok())
                {
                    Some(rel) if !rel.is_empty() => rel,
                    _ => ".".to_string(),
                }
            }
        };

        let args = paths::split_args(args);
        let env = crate::linux_gaming::validate_env(&options.env)?;
        let mode = PortableMode::detect(Path::new(&executable));

        Ok(Self {
            title: installed.title.clone(),
            version: installed.version.clone().unwrap_or_default(),
            slug: paths::slug_for_title(&installed.title),
            executable,
            working_dir,
            args,
            env,
            mode,
            gameid: options
                .steam_app_id
                .map(|app_id| app_id.to_string())
                .unwrap_or_else(|| "0".to_string()),
            prefix: PREFIX_DIR.to_string(),
            script_version: SCRIPT_VERSION,
            generated_at: now.to_string(),
            generator: generator.to_string(),
        })
    }

    pub fn runtime(&self) -> RuntimeRequirement {
        RuntimeRequirement::for_mode(self.mode)
    }

    /// Reject anything that could break out of the bundle, the JSON or the
    /// generated shell. Bounded and control-character free.
    pub fn validate(&self) -> Result<(), String> {
        validate_text("The game title", &self.title, MAX_TITLE_CHARS)?;
        if !self.version.is_empty() {
            validate_text("The version", &self.version, MAX_VERSION_CHARS)?;
        }
        validate_slug(&self.slug)?;
        let executable = validate_relative("The executable", &self.executable)?;
        if executable.is_empty() {
            return Err("The executable path is empty.".to_string());
        }
        if self.working_dir != "." {
            validate_relative("The working folder", &self.working_dir)?;
        }
        validate_relative("The prefix folder", &self.prefix)?;
        if self.script_version != SCRIPT_VERSION {
            return Err(format!(
                "This bundle generator writes script version {SCRIPT_VERSION}, not {}.",
                self.script_version
            ));
        }
        if self.args.len() > MAX_ARGS {
            return Err(format!(
                "Too many launch arguments ({}, at most {MAX_ARGS}).",
                self.args.len()
            ));
        }
        for argument in &self.args {
            validate_text("A launch argument", argument, MAX_ARG_CHARS)?;
        }
        if !self.gameid.chars().all(|c| c.is_ascii_digit()) || self.gameid.len() > 10 {
            return Err("The Steam app id must be a number.".to_string());
        }
        validate_text("The generation time", &self.generated_at, 64)?;
        validate_text("The generator name", &self.generator, 64)?;
        validate_env(&self.env)
    }
}

fn validate_text(label: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} is empty."));
    }
    if value.chars().count() > max {
        return Err(format!("{label} is longer than {max} characters."));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "{label} contains a control character (newline, tab and NUL are refused)."
        ));
    }
    Ok(())
}

fn validate_slug(slug: &str) -> Result<(), String> {
    let valid = !slug.is_empty()
        && slug.chars().count() <= MAX_SLUG_CHARS
        && slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        && slug.starts_with(|character: char| {
            character.is_ascii_lowercase() || character.is_ascii_digit()
        })
        && !slug.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "The slug {slug:?} must be lowercase letters, digits and dashes, bounded to {MAX_SLUG_CHARS} characters."
        ))
    }
}

/// Normalise a relative path to `/`-separated components and refuse escapes.
fn validate_relative(label: &str, value: &str) -> Result<String, String> {
    if value.chars().count() > MAX_PATH_CHARS {
        return Err(format!(
            "{label} is longer than {MAX_PATH_CHARS} characters."
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} contains a control character."));
    }
    relative_path(Path::new(value)).map_err(|error| format!("{label}: {error}"))
}

fn relative_path(path: &Path) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.is_empty() {
                    return Err("A path component is empty.".to_string());
                }
                parts.push(part.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err("Relative paths must not contain '..'.".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(
                    "The path must stay inside the game folder (no absolute path).".to_string(),
                );
            }
        }
    }
    Ok(parts.join("/"))
}

/// Variables the bundle itself owns. A per-game value would move the prefix out
/// of the folder or point the compatibility tool elsewhere, so it is refused
/// instead of silently ignored.
const BUNDLE_ENV: [&str; 2] = ["WINEPREFIX", "STEAM_COMPAT_DATA_PATH"];

/// Environment overrides are already validated for a launch; the bundle adds
/// its own rule: no `LUDEX_*` key may replace the script's own decisions, and
/// the variables the scripts own stay theirs.
fn validate_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (key, value) in env {
        if key.starts_with("LUDEX_") {
            return Err(format!(
                "{key} is reserved by the portable bundle and cannot be overridden."
            ));
        }
        if BUNDLE_ENV.contains(&key.as_str()) {
            return Err(format!(
                "{key} is set by the portable bundle itself (the prefix lives in the folder) and cannot be overridden."
            ));
        }
        if value.chars().count() > 4096 {
            return Err(format!(
                "The value of {key} is longer than 4096 characters."
            ));
        }
        if value.contains('\0') || value.contains('\n') || value.contains('\r') {
            return Err(format!("The value of {key} contains a line break or NUL."));
        }
    }
    Ok(())
}

/* ---------------------------------------------------------------------- */
/* Shell quoting and rendering                                             */
/* ---------------------------------------------------------------------- */

/// Quote one value for `bash`: bare when it is already safe, single-quoted
/// otherwise, with the `'\''` idiom for embedded quotes.
pub fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(
                    character,
                    '_' | '-' | '.' | '/' | ':' | ',' | '@' | '%' | '+' | '='
                )
        });
    if safe {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The shared prologue: where the file came from, strict mode and the constants
/// both scripts use.
fn render_preamble(out: &mut String, spec: &PortableSpec, script: &str) {
    let version = if spec.version.is_empty() {
        String::new()
    } else {
        format!(" {}", spec.version)
    };
    let _ = writeln!(out, "#!/usr/bin/env bash");
    let _ = writeln!(out, "# Ludex portable bundle: {}{version}", spec.title);
    let _ = writeln!(out, "#");
    let _ = writeln!(
        out,
        "# Generated by {} for a Linux install (#332). The folder is the game:",
        spec.generator
    );
    let _ = writeln!(
        out,
        "# it can be moved or copied to another Linux machine and started without"
    );
    let _ = writeln!(
        out,
        "# Ludex. Nothing in this file refers to Ludex, to a home directory or to"
    );
    let _ = writeln!(out, "# the machine it was generated on.");
    let _ = writeln!(out, "#");
    if script == RUN_SCRIPT {
        let _ = writeln!(
            out,
            "# Start the game with:      ./run.sh [extra arguments]"
        );
        let _ = writeln!(out, "# First time somewhere new: ./install.sh");
    } else {
        let _ = writeln!(out, "# Prepare the folder:      ./install.sh");
        let _ = writeln!(out, "# Then start the game:     ./run.sh [extra arguments]");
    }
    let _ = writeln!(out, "set -euo pipefail");
    let _ = writeln!(out);
    let _ = writeln!(out, "LUDEX_SCRIPT_VERSION={}", spec.script_version);
    let _ = writeln!(out, "LUDEX_SLUG={}", shell_quote(&spec.slug));
    let _ = writeln!(out, "LUDEX_MODE={}", shell_quote(spec.mode.as_str()));
    let _ = writeln!(out, "LUDEX_EXECUTABLE={}", shell_quote(&spec.executable));
    let _ = writeln!(out, "LUDEX_WORKING_DIR={}", shell_quote(&spec.working_dir));
    let _ = writeln!(out, "LUDEX_PREFIX={}", shell_quote(&spec.prefix));
    let _ = writeln!(out, "LUDEX_GAMEID={}", shell_quote(&spec.gameid));
    let _ = writeln!(out);
}

/// Resolve the real folder, following a symlinked script, without leaving a `cd`
/// behind. Both scripts carry the same block so each stands alone.
fn render_script_dir(out: &mut String, script: &str) {
    let _ = writeln!(out, "SOURCE=\"${{BASH_SOURCE[0]}}\"");
    let _ = writeln!(out, "while [ -h \"$SOURCE\" ]; do");
    let _ = writeln!(out, "  TARGET=\"$(readlink -- \"$SOURCE\")\"");
    let _ = writeln!(out, "  case \"$TARGET\" in");
    let _ = writeln!(out, "    /*) SOURCE=\"$TARGET\" ;;");
    let _ = writeln!(
        out,
        "    *) SOURCE=\"$(dirname -- \"$SOURCE\")/$TARGET\" ;;"
    );
    let _ = writeln!(out, "  esac");
    let _ = writeln!(out, "done");
    let _ = writeln!(
        out,
        "SCRIPT_DIR=\"$(cd -P -- \"$(dirname -- \"$SOURCE\")\" && pwd -P)\""
    );
    let _ = writeln!(out, "unset SOURCE TARGET");
    let _ = writeln!(out);
    let name = shell_quote(script);
    // Informational lines go to stdout (a normal CLI), warnings and errors to
    // stderr so a caller can tell the difference. `run.sh` only talks when
    // something is wrong.
    if script == INSTALL_SCRIPT {
        let _ = writeln!(out, "say() {{ printf '%s: %s\\n' {name} \"$*\"; }}");
        let _ = writeln!(
            out,
            "warn() {{ printf '%s: warning: %s\\n' {name} \"$*\" >&2; }}"
        );
    }
    let _ = writeln!(
        out,
        "die() {{ printf '%s: %s\\n' {name} \"$*\" >&2; exit 1; }}"
    );
    let _ = writeln!(out);
}

/// The manifest check both scripts run: the JSON must be here, must be this
/// game and must be the same script version. No JSON parser is required.
fn render_manifest_guard(out: &mut String) {
    let _ = writeln!(out, "MANIFEST=\"$SCRIPT_DIR/ludex-game.json\"");
    let _ = writeln!(out, "if [ ! -f \"$MANIFEST\" ]; then");
    let _ = writeln!(
        out,
        "  die \"ludex-game.json is missing. Re-create the bundle in Ludex, or run ./install.sh from a complete copy.\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(
        out,
        "if ! grep -Eq \"\\\"slug\\\"[[:space:]]*:[[:space:]]*\\\"$LUDEX_SLUG\\\"\" \"$MANIFEST\"; then"
    );
    let _ = writeln!(
        out,
        "  die \"ludex-game.json describes another game (expected $LUDEX_SLUG). Refusing to start it.\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(
        out,
        "if ! grep -Eq \"\\\"scriptVersion\\\"[[:space:]]*:[[:space:]]*$LUDEX_SCRIPT_VERSION([^0-9]|\\$)\" \"$MANIFEST\"; then"
    );
    let _ = writeln!(
        out,
        "  die \"ludex-game.json was written by another bundle version. Re-create the bundle in Ludex.\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
}

fn render_common_paths(out: &mut String) {
    let _ = writeln!(out, "GAME_EXE=\"$SCRIPT_DIR/$LUDEX_EXECUTABLE\"");
    let _ = writeln!(out, "if [ ! -f \"$GAME_EXE\" ]; then");
    let _ = writeln!(
        out,
        "  die \"The game file $LUDEX_EXECUTABLE is missing. Copy the whole folder again or run ./install.sh.\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out, "GAME_DIR=\"$SCRIPT_DIR/$LUDEX_WORKING_DIR\"");
    let _ = writeln!(out, "if [ ! -d \"$GAME_DIR\" ]; then");
    let _ = writeln!(
        out,
        "  die \"The working folder $LUDEX_WORKING_DIR is missing from this bundle.\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
}

/// The runtime probe shared by both scripts: an explicit `PROTONPATH`, the
/// bundled runtime, Steam's `compatibilitytools.d`, then Steam libraries.
/// Prints the newest matching Proton directory and returns 0, or prints nothing
/// and returns 1.
fn render_proton_probe(out: &mut String) {
    let _ = writeln!(
        out,
        "# Newest installed Proton build, as a folder containing the `proton` script."
    );
    let _ = writeln!(out, "proton_dir() {{");
    let _ = writeln!(out, "  local base dir");
    let _ = writeln!(out, "  local home_dir=\"${{HOME:-}}\"");
    let _ = writeln!(
        out,
        "  if [ -n \"${{PROTONPATH:-}}\" ] && [ -f \"$PROTONPATH/proton\" ]; then"
    );
    let _ = writeln!(out, "    printf '%s\\n' \"$PROTONPATH\"");
    let _ = writeln!(out, "    return 0");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(
        out,
        "  if [ -f \"$SCRIPT_DIR/support/runtime/proton\" ]; then"
    );
    let _ = writeln!(out, "    printf '%s\\n' \"$SCRIPT_DIR/support/runtime\"");
    let _ = writeln!(out, "    return 0");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  for base in \\");
    let _ = writeln!(
        out,
        "    \"$home_dir/.local/share/Steam/compatibilitytools.d\" \\"
    );
    let _ = writeln!(
        out,
        "    \"$home_dir/.steam/steam/compatibilitytools.d\" \\"
    );
    let _ = writeln!(
        out,
        "    \"$home_dir/.steam/root/compatibilitytools.d\"; do"
    );
    let _ = writeln!(out, "    [ -d \"$base\" ] || continue");
    let _ = writeln!(
        out,
        "    dir=\"$(find \"$base\" -maxdepth 2 -type f -name proton 2>/dev/null | sort -V | tail -n 1 || true)\""
    );
    let _ = writeln!(out, "    if [ -n \"$dir\" ]; then");
    let _ = writeln!(out, "      dirname -- \"$dir\"");
    let _ = writeln!(out, "      return 0");
    let _ = writeln!(out, "    fi");
    let _ = writeln!(out, "  done");
    let _ = writeln!(out, "  for base in \\");
    let _ = writeln!(
        out,
        "    \"$home_dir/.local/share/Steam/steamapps/common\" \\"
    );
    let _ = writeln!(out, "    \"$home_dir/.steam/steam/steamapps/common\"; do");
    let _ = writeln!(out, "    [ -d \"$base\" ] || continue");
    let _ = writeln!(
        out,
        "    dir=\"$(find \"$base\" -maxdepth 2 -type f -name proton 2>/dev/null | sort -V | tail -n 1 || true)\""
    );
    let _ = writeln!(out, "    if [ -n \"$dir\" ]; then");
    let _ = writeln!(out, "      dirname -- \"$dir\"");
    let _ = writeln!(out, "      return 0");
    let _ = writeln!(out, "    fi");
    let _ = writeln!(out, "  done");
    let _ = writeln!(out, "  return 1");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// The shared missing-runtime text. Printed, never executed: the `sudo` lines
/// are advice for the person reading the terminal, not commands this script
/// runs.
fn render_missing_runtime(out: &mut String, function: &str) {
    let _ = writeln!(
        out,
        "# The runtime is missing on this machine: explain, never install."
    );
    let _ = writeln!(out, "{function}() {{");
    let _ = writeln!(out, "  printf '%s\\n' \\");
    let _ = writeln!(out, "    \"\" \\");
    let _ = writeln!(
        out,
        "    \"This is a Windows game and this machine cannot run it yet.\" \\"
    );
    let _ = writeln!(
        out,
        "    \"Install one of these, then run ./install.sh again:\" \\"
    );
    let _ = writeln!(out, "    \"\" \\");
    let _ = writeln!(
        out,
        "    \"  UMU (recommended: it manages Proton for you)\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    Arch:          sudo pacman -S umu-launcher\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    Fedora:        sudo dnf install umu-launcher\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    Debian/Ubuntu: sudo apt install umu-launcher\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    Anything else: https://github.com/Open-Wine-Components/umu-launcher/releases\" \\"
    );
    let _ = writeln!(out, "    \"\" \\");
    let _ = writeln!(
        out,
        "    \"  GE-Proton / Proton (works well when Steam is installed)\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    Steam > Settings > Compatibility > enable Steam Play, or\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    https://github.com/GloriousEggroll/proton-ge-custom/releases\" \\"
    );
    let _ = writeln!(
        out,
        "    \"    and put the folder in ~/.local/share/Steam/compatibilitytools.d\" \\"
    );
    let _ = writeln!(out, "    \"\" \\");
    let _ = writeln!(out, "    \"  Wine (last resort)\" \\");
    let _ = writeln!(out, "    \"    Arch:          sudo pacman -S wine\" \\");
    let _ = writeln!(out, "    \"    Fedora:        sudo dnf install wine\" \\");
    let _ = writeln!(out, "    \"    Debian/Ubuntu: sudo apt install wine\" \\");
    let _ = writeln!(out, "    \"\" \\");
    let _ = writeln!(
        out,
        "    \"Nothing was installed and nothing was started by this script.\" \\"
    );
    let _ = writeln!(out, "    \"No download is attempted for you.\" \\");
    let _ = writeln!(out, "    >&2");
    let _ = writeln!(out, "  exit 1");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
}

/// `run.sh`: locate the folder, verify the manifest, then `exec` the game.
pub fn render_run_script(spec: &PortableSpec) -> String {
    let mut out = String::new();
    render_preamble(&mut out, spec, RUN_SCRIPT);
    render_script_dir(&mut out, RUN_SCRIPT);
    render_manifest_guard(&mut out);
    render_common_paths(&mut out);
    let _ = writeln!(out, "cd \"$GAME_DIR\"");
    let _ = writeln!(out);
    let args = if spec.args.is_empty() {
        "LUDEX_ARGS=()".to_string()
    } else {
        let quoted = spec
            .args
            .iter()
            .map(|argument| shell_quote(argument))
            .collect::<Vec<_>>()
            .join(" ");
        format!("LUDEX_ARGS=({quoted})")
    };
    let _ = writeln!(
        out,
        "# Arguments recorded when the bundle was generated; extra ones are appended."
    );
    let _ = writeln!(out, "{args}");
    let _ = writeln!(out);
    let _ = writeln!(out, "if [ \"$LUDEX_MODE\" != \"windows\" ]; then");
    let _ = writeln!(
        out,
        "  # Native Linux build: no compatibility layer. Libraries that ship in"
    );
    let _ = writeln!(
        out,
        "  # support/lib (and support/lib64) are used first so a GOG-style tree keeps"
    );
    let _ = writeln!(out, "  # its own copies working.");
    let _ = writeln!(out, "  if [ -d \"$SCRIPT_DIR/support/lib\" ]; then");
    let _ = writeln!(
        out,
        "    LD_LIBRARY_PATH=\"$SCRIPT_DIR/support/lib${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\""
    );
    let _ = writeln!(out, "    export LD_LIBRARY_PATH");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  if [ -d \"$SCRIPT_DIR/support/lib64\" ]; then");
    let _ = writeln!(
        out,
        "    LD_LIBRARY_PATH=\"$SCRIPT_DIR/support/lib64${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\""
    );
    let _ = writeln!(out, "    export LD_LIBRARY_PATH");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  if [ ! -x \"$GAME_EXE\" ]; then");
    let _ = writeln!(
        out,
        "    chmod +x -- \"$GAME_EXE\" 2>/dev/null || die \"The game file is not executable. Run ./install.sh.\""
    );
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  exec \"$GAME_EXE\" \"${{LUDEX_ARGS[@]}}\" \"$@\"");
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "# Windows build: the prefix lives inside this folder, so the whole bundle"
    );
    let _ = writeln!(out, "# moves as one unit.");
    let _ = writeln!(out, "WINEPREFIX=\"$SCRIPT_DIR/$LUDEX_PREFIX\"");
    let _ = writeln!(out, "export WINEPREFIX");
    let _ = writeln!(out, "export STEAM_COMPAT_DATA_PATH=\"$WINEPREFIX\"");
    let _ = writeln!(out, "export GAMEID=\"$LUDEX_GAMEID\"");
    let _ = writeln!(out, "export UMU_LOG=\"${{UMU_LOG:-warn}}\"");
    let _ = writeln!(out, "mkdir -p -- \"$WINEPREFIX\"");
    for (key, value) in &spec.env {
        let _ = writeln!(out, "export {key}={}", shell_quote(value));
    }
    let _ = writeln!(out);
    render_proton_probe(&mut out);
    render_missing_runtime(&mut out, "missing_runtime");
    let _ = writeln!(
        out,
        "# Runtime order: bundled support/runtime, umu-run, GE-Proton/Proton, wine."
    );
    let _ = writeln!(
        out,
        "if [ -x \"$SCRIPT_DIR/support/runtime/umu-run\" ]; then"
    );
    let _ = writeln!(out, "  PROTON=\"$(proton_dir || true)\"");
    let _ = writeln!(
        out,
        "  export PROTONPATH=\"${{PROTON:-${{PROTONPATH:-GE-Proton}}}}\""
    );
    let _ = writeln!(
        out,
        "  exec \"$SCRIPT_DIR/support/runtime/umu-run\" \"$GAME_EXE\" \"${{LUDEX_ARGS[@]}}\" \"$@\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "if [ -x \"$SCRIPT_DIR/support/runtime/bin/wine\" ]; then"
    );
    let _ = writeln!(
        out,
        "  export STEAM_COMPAT_CLIENT_INSTALL_PATH=\"${{STEAM_COMPAT_CLIENT_INSTALL_PATH:-${{HOME:-}}/.local/share/Steam}}\""
    );
    let _ = writeln!(
        out,
        "  exec \"$SCRIPT_DIR/support/runtime/bin/wine\" \"$GAME_EXE\" \"${{LUDEX_ARGS[@]}}\" \"$@\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(out, "if command -v umu-run >/dev/null 2>&1; then");
    let _ = writeln!(out, "  PROTON=\"$(proton_dir || true)\"");
    let _ = writeln!(
        out,
        "  export PROTONPATH=\"${{PROTON:-${{PROTONPATH:-GE-Proton}}}}\""
    );
    let _ = writeln!(
        out,
        "  exec umu-run \"$GAME_EXE\" \"${{LUDEX_ARGS[@]}}\" \"$@\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(out, "PROTON=\"$(proton_dir || true)\"");
    let _ = writeln!(out, "if [ -n \"$PROTON\" ]; then");
    let _ = writeln!(
        out,
        "  export STEAM_COMPAT_CLIENT_INSTALL_PATH=\"${{STEAM_COMPAT_CLIENT_INSTALL_PATH:-${{HOME:-}}/.local/share/Steam}}\""
    );
    let _ = writeln!(
        out,
        "  exec \"$PROTON/proton\" run \"$GAME_EXE\" \"${{LUDEX_ARGS[@]}}\" \"$@\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(out, "if command -v wine >/dev/null 2>&1; then");
    let _ = writeln!(
        out,
        "  exec wine \"$GAME_EXE\" \"${{LUDEX_ARGS[@]}}\" \"$@\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(out, "missing_runtime");
    out
}

/// `install.sh`: make the folder ready, without root and without surprises.
pub fn render_install_script(spec: &PortableSpec) -> String {
    let mut out = String::new();
    render_preamble(&mut out, spec, INSTALL_SCRIPT);
    render_script_dir(&mut out, INSTALL_SCRIPT);
    render_manifest_guard(&mut out);
    render_common_paths(&mut out);
    let _ = writeln!(out, "say \"Portable bundle: {}\"", spec.title);
    let _ = writeln!(out, "say \"Folder: $SCRIPT_DIR\"");
    let _ = writeln!(out);
    let _ = writeln!(out, "# 1. Executable bits. Idempotent by nature:");
    let _ = writeln!(out, "#    chmod is a no-op when the mode is already right.");
    let _ = writeln!(
        out,
        "chmod +x -- \"$SCRIPT_DIR/run.sh\" \"$SCRIPT_DIR/install.sh\" 2>/dev/null || true"
    );
    let _ = writeln!(out, "chmod +x -- \"$GAME_EXE\" 2>/dev/null || true");
    let _ = writeln!(out, "say \"Scripts and game file: executable\"");
    let _ = writeln!(out);
    let _ = writeln!(out, "# 2. Files the manifest promises are here.");
    let _ = writeln!(out, "for required in ludex-game.json run.sh install.sh; do");
    let _ = writeln!(out, "  if [ ! -f \"$SCRIPT_DIR/$required\" ]; then");
    let _ = writeln!(
        out,
        "    die \"This copy is incomplete: $required is missing.\""
    );
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "done");
    let _ = writeln!(out, "say \"Recorded files: ok\"");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "# 3. Optional checksums, when a tool wrote {MANIFEST_SUMS}."
    );
    let _ = writeln!(out, "if [ -f \"$SCRIPT_DIR/{MANIFEST_SUMS}\" ]; then");
    let _ = writeln!(out, "  if command -v sha256sum >/dev/null 2>&1; then");
    let _ = writeln!(
        out,
        "    if ( cd \"$SCRIPT_DIR\" && sha256sum -c --quiet {MANIFEST_SUMS} >/dev/null 2>&1 ); then"
    );
    let _ = writeln!(out, "      say \"Checksums: ok\"");
    let _ = writeln!(out, "    else");
    let _ = writeln!(
        out,
        "      die \"The game files do not match {MANIFEST_SUMS}. Copy the folder again.\""
    );
    let _ = writeln!(out, "    fi");
    let _ = writeln!(out, "  else");
    let _ = writeln!(
        out,
        "    warn \"sha256sum is not installed; the checksum list was not verified.\""
    );
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    render_proton_probe(&mut out);
    render_missing_runtime(&mut out, "missing_runtime");
    let _ = writeln!(out, "RUNTIME='none'");
    let _ = writeln!(out, "RUNTIME_LABEL=''");
    let _ = writeln!(out, "if [ \"$LUDEX_MODE\" = \"windows\" ]; then");
    let _ = writeln!(
        out,
        "  if [ -x \"$SCRIPT_DIR/support/runtime/umu-run\" ]; then"
    );
    let _ = writeln!(out, "    RUNTIME='umu'");
    let _ = writeln!(out, "    RUNTIME_LABEL='the bundled UMU'");
    let _ = writeln!(out, "  elif command -v umu-run >/dev/null 2>&1; then");
    let _ = writeln!(out, "    RUNTIME='umu'");
    let _ = writeln!(out, "    RUNTIME_LABEL='umu-run'");
    let _ = writeln!(
        out,
        "  elif [ -x \"$SCRIPT_DIR/support/runtime/bin/wine\" ]; then"
    );
    let _ = writeln!(out, "    RUNTIME='wine'");
    let _ = writeln!(out, "    RUNTIME_LABEL='the bundled Wine'");
    let _ = writeln!(
        out,
        "  elif PROTON=\"$(proton_dir || true)\" && [ -n \"$PROTON\" ]; then"
    );
    let _ = writeln!(out, "    RUNTIME='proton'");
    let _ = writeln!(out, "    RUNTIME_LABEL='Proton'");
    let _ = writeln!(out, "  elif command -v wine >/dev/null 2>&1; then");
    let _ = writeln!(out, "    RUNTIME='wine'");
    let _ = writeln!(out, "    RUNTIME_LABEL='Wine'");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  if [ \"$RUNTIME\" = 'none' ]; then");
    let _ = writeln!(out, "    missing_runtime");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  say \"Compatibility runtime: $RUNTIME_LABEL\"");
    let _ = writeln!(out, "else");
    let _ = writeln!(
        out,
        "  say \"Native Linux build: no compatibility runtime needed\""
    );
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "# 4. The prefix stays inside the bundle so the folder stays movable."
    );
    let _ = writeln!(out, "PREFIX=\"$SCRIPT_DIR/$LUDEX_PREFIX\"");
    let _ = writeln!(out, "mkdir -p -- \"$PREFIX\"");
    let _ = writeln!(out, "say \"Prefix: $LUDEX_PREFIX\"");
    let _ = writeln!(out);
    let _ = writeln!(out, "init_prefix() {{");
    let _ = writeln!(out, "  local proton wine_bin=''");
    let _ = writeln!(out, "  proton=\"$(proton_dir || true)\"");
    let _ = writeln!(
        out,
        "  if [ -x \"$SCRIPT_DIR/support/runtime/bin/wine\" ]; then"
    );
    let _ = writeln!(out, "    wine_bin=\"$SCRIPT_DIR/support/runtime/bin/wine\"");
    let _ = writeln!(
        out,
        "  elif [ -n \"$proton\" ] && [ -x \"$proton/files/bin/wine\" ]; then"
    );
    let _ = writeln!(out, "    wine_bin=\"$proton/files/bin/wine\"");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  if [ -n \"$wine_bin\" ]; then");
    let _ = writeln!(
        out,
        "    WINEPREFIX=\"$PREFIX\" STEAM_COMPAT_DATA_PATH=\"$PREFIX\" \"$wine_bin\" wineboot -u >/dev/null 2>&1 && return 0"
    );
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "  case \"$RUNTIME\" in");
    let _ = writeln!(out, "    umu)");
    let _ = writeln!(
        out,
        "      if [ -x \"$SCRIPT_DIR/support/runtime/umu-run\" ]; then"
    );
    let _ = writeln!(
        out,
        "        PROTONPATH=\"${{proton:-GE-Proton}}\" GAMEID=\"$LUDEX_GAMEID\" UMU_LOG=warn \\"
    );
    let _ = writeln!(
        out,
        "          WINEPREFIX=\"$PREFIX\" STEAM_COMPAT_DATA_PATH=\"$PREFIX\" \\"
    );
    let _ = writeln!(
        out,
        "          \"$SCRIPT_DIR/support/runtime/umu-run\" wineboot -u >/dev/null 2>&1 && return 0"
    );
    let _ = writeln!(out, "      fi");
    let _ = writeln!(out, "      if command -v umu-run >/dev/null 2>&1; then");
    let _ = writeln!(
        out,
        "        PROTONPATH=\"${{proton:-GE-Proton}}\" GAMEID=\"$LUDEX_GAMEID\" UMU_LOG=warn \\"
    );
    let _ = writeln!(
        out,
        "          WINEPREFIX=\"$PREFIX\" STEAM_COMPAT_DATA_PATH=\"$PREFIX\" \\"
    );
    let _ = writeln!(
        out,
        "          umu-run wineboot -u >/dev/null 2>&1 && return 0"
    );
    let _ = writeln!(out, "      fi");
    let _ = writeln!(out, "      ;;");
    let _ = writeln!(out, "    wine)");
    let _ = writeln!(
        out,
        "      WINEPREFIX=\"$PREFIX\" wine wineboot -u >/dev/null 2>&1 && return 0"
    );
    let _ = writeln!(out, "      ;;");
    let _ = writeln!(out, "    *)");
    let _ = writeln!(out, "      ;;");
    let _ = writeln!(out, "  esac");
    let _ = writeln!(out, "  return 1");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "if [ \"$RUNTIME\" != 'none' ]; then");
    let _ = writeln!(out, "  if [ -f \"$PREFIX/.ludex-initialised\" ]; then");
    let _ = writeln!(out, "    say \"Prefix: already initialised\"");
    let _ = writeln!(out, "  else");
    let _ = writeln!(
        out,
        "    say \"Initialising the prefix with $RUNTIME_LABEL (one time)...\""
    );
    let _ = writeln!(out, "    if init_prefix; then");
    let _ = writeln!(
        out,
        "      printf '%s\\n' \"yes\" > \"$PREFIX/.ludex-initialised\""
    );
    let _ = writeln!(out, "      say \"Prefix: ready\"");
    let _ = writeln!(out, "    else");
    let _ = writeln!(
        out,
        "      warn \"The prefix was not initialised. ./run.sh still creates it on the first start; run this script again if the game does not start.\""
    );
    let _ = writeln!(out, "    fi");
    let _ = writeln!(out, "  fi");
    let _ = writeln!(out, "fi");
    let _ = writeln!(out);
    let _ = writeln!(out, "say \"Done. Start the game with: ./run.sh\"");
    out
}

/// `ludex-game.json`: the folder's own description, readable by a person and by
/// a future importer. No secret, no absolute path.
pub fn render_manifest(spec: &PortableSpec) -> String {
    let value = serde_json::json!({
        "schema": 1,
        "kind": "ludex-portable-bundle",
        "title": spec.title,
        "version": spec.version,
        "slug": spec.slug,
        "executable": spec.executable,
        "workingDir": spec.working_dir,
        "args": spec.args,
        "env": spec.env,
        "launchMode": spec.mode.as_str(),
        "runtimeRequirement": spec.runtime().as_str(),
        "prefix": spec.prefix,
        "scriptVersion": spec.script_version,
        "generatedAt": spec.generated_at,
        "generator": spec.generator,
    });
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
}

/// `support/README.txt`: what is portable, what the target needs, how to move
/// it, and how to regenerate it.
pub fn render_readme(spec: &PortableSpec) -> String {
    let version = if spec.version.is_empty() {
        "not recorded"
    } else {
        spec.version.as_str()
    };
    let (kind_text, needs_text, runtime_text) = match spec.mode {
        PortableMode::Native => (
            "native Linux build",
            "A 64-bit Linux machine with the libraries this build links against (glibc, \
             graphics drivers, audio). Same architecture and a reasonably current distribution: \
             nothing else is required.",
            "None. No compatibility runtime is installed or needed.",
        ),
        PortableMode::Windows => (
            "Windows build",
            "A Linux machine with one of: UMU (umu-launcher, recommended), GE-Proton/Proton in \
             Steam, or Wine. ./install.sh names the exact packages for Arch, Fedora and \
             Debian/Ubuntu when none is found, and exits with an error instead of pretending \
             the folder is ready.",
            "An existing UMU, Proton or Wine installation, unless a runtime already sits in \
             support/runtime.",
        ),
    };
    format!(
        "{title}\n\
         {underline}\n\
         \n\
         Version: {version}\n\
         Kind: {kind_text}\n\
         Generated by {generator} on {generated_at}.\n\
         \n\
         THIS FOLDER IS THE GAME\n\
         \n\
         It is self-contained: run.sh, install.sh, ludex-game.json and support/ live next to\n\
         the game files. Nothing refers to Ludex, to the user's home directory or to the\n\
         machine the bundle came from. Every path is resolved from the folder it sits in, so\n\
         the folder keeps working after a move, a rename, or a copy to another machine and\n\
         another user.\n\
         \n\
         START IT\n\
         \n\
           ./install.sh        # once per copy: executable bits, checks, prefix\n\
           ./run.sh            # start the game; extra arguments are passed through\n\
         \n\
         WHAT THE TARGET MACHINE NEEDS\n\
         \n\
         {needs_text}\n\
         \n\
         Runtime: {runtime_text}\n\
         \n\
         HOW TO MOVE IT\n\
         \n\
         Copy or move the whole folder, keeping the file modes:\n\
         \n\
           cp -a \"{title}\" /media/usb/Games/\n\
           cd /media/usb/Games/{title}\n\
           ./install.sh\n\
           ./run.sh\n\
         \n\
         Do not merge two copies of the same game into one folder, and do not copy only the\n\
         executable: run.sh needs ludex-game.json and support/ next to it.\n\
         \n\
         WHAT IS NOT INCLUDED\n\
         \n\
         - Ludex itself: the game starts without it.\n\
         - Saves and configuration that live outside this folder. Cloud saves and per-game\n\
           settings are managed by Ludex, not by the bundle.\n\
         - A compatibility runtime, unless support/runtime exists in this copy. Nothing is\n\
           downloaded at run time.\n\
         - Checksums, unless support/manifest.sha256 exists. install.sh verifies that file\n\
           when it is present.\n\
         \n\
         THE PREFIX\n\
         \n\
         A Windows build uses {prefix} inside the bundle. That keeps the folder movable, but\n\
         a Wine prefix is tied to the machine that created it: after copying the folder to\n\
         another computer, delete support/prefix and run ./install.sh again to build a fresh\n\
         one.\n\
         \n\
         HOW TO REGENERATE\n\
         \n\
         In Ludex: open the game, Files, then \"Create portable Linux bundle\". Regenerating\n\
         rewrites run.sh, install.sh, ludex-game.json and this README; the game files are not\n\
         touched.\n",
        title = spec.title,
        underline = "=".repeat(spec.title.chars().count().max(4)),
        version = version,
        kind_text = kind_text,
        generator = spec.generator,
        generated_at = spec.generated_at,
        needs_text = needs_text,
        runtime_text = runtime_text,
        prefix = spec.prefix,
    )
}

/* ---------------------------------------------------------------------- */
/* Writing                                                                 */
/* ---------------------------------------------------------------------- */

/// What one generation wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableBundle {
    pub dir: PathBuf,
    /// Files written, relative to `dir`, in write order.
    pub files: Vec<String>,
    /// Bytes of the four generated files.
    pub bytes: u64,
    pub runtime: RuntimeRequirement,
    /// Files that existed before generation and were replaced (an archive
    /// shipped its own `run.sh`, for instance).
    pub replaced: Vec<String>,
}

/// Write the four generated files into `dir`.
///
/// Only these four paths are touched, all directly under `dir`; the game files
/// are never read or rewritten. Generated files always win over same-named files
/// the archive shipped, and the replacement is reported.
pub fn generate(dir: &Path, spec: &PortableSpec) -> Result<PortableBundle, String> {
    spec.validate()?;
    if dir.as_os_str().is_empty() {
        return Err("The bundle folder is empty.".to_string());
    }
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;

    let mut replaced = Vec::new();
    let mut files = Vec::new();
    let mut bytes = 0u64;

    let run = render_run_script(spec);
    let install = render_install_script(spec);
    let manifest = render_manifest(spec);
    let readme = render_readme(spec);

    for (relative, content, mode) in [
        (RUN_SCRIPT, run.as_str(), 0o755),
        (INSTALL_SCRIPT, install.as_str(), 0o755),
        (MANIFEST_FILE, manifest.as_str(), 0o644),
        (README_FILE, readme.as_str(), 0o644),
    ] {
        let path = dir.join(relative);
        if path.exists() {
            replaced.push(relative.to_string());
            tracing::warn!(
                path = %path.display(),
                "an existing portable file is replaced by the generated one"
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
        set_mode(&path, mode);
        bytes = bytes.saturating_add(content.len() as u64);
        files.push(relative.to_string());
    }

    Ok(PortableBundle {
        dir: dir.to_path_buf(),
        files,
        bytes,
        runtime: spec.runtime(),
        replaced,
    })
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

/// Size of the folder as it would be copied: the two Wine prefixes are skipped
/// because they are machine-specific and recreated by `install.sh`.
pub fn bundle_size(root: &Path) -> u64 {
    dir_size_skipping_prefixes(root).0
}

fn dir_size_skipping_prefixes(root: &Path) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.is_dir() {
            if is_skipped_dir(root, &current) {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&current) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        } else if metadata.is_file() {
            bytes = bytes.saturating_add(metadata.len());
            files += 1;
        }
    }
    (bytes, files)
}

/// The default Ludex prefix and the bundle prefix never travel.
fn is_skipped_dir(root: &Path, path: &Path) -> bool {
    if path == root {
        return false;
    }
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    relative == Path::new(".prefix") || relative == Path::new(PREFIX_DIR)
}

/* ---------------------------------------------------------------------- */
/* Commands                                                                */
/* ---------------------------------------------------------------------- */

/// Wire format of the requirement, for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableEstimate {
    /// False off Linux: the action is hidden instead of failing.
    pub supported: bool,
    pub platform: String,
    pub game_id: i64,
    /// Bytes the copy would write (prefixes excluded).
    pub bytes: u64,
    /// `linux-native` or `windows-compat`.
    pub runtime_requirement: String,
    /// The recorded executable was found inside the install folder.
    pub executable_present: bool,
    /// Why the action cannot run, when it cannot.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableBundleInfo {
    pub path: String,
    pub bytes: u64,
    pub runtime_required: String,
    pub file_count: u32,
    /// Existing files the generation replaced.
    pub replaced: Vec<String>,
    /// True when the install folder was copied to `path` first.
    pub copied: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableProgress {
    pub game_id: i64,
    /// `copying`, `generating`, `done` or `failed`.
    pub phase: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PortableStatus {
    game_id: i64,
    ok: bool,
    message: String,
}

/// Portable bundles are a Linux feature; elsewhere the UI hides the action.
pub fn supported() -> bool {
    cfg!(target_os = "linux")
}

fn generator_label() -> String {
    format!("Ludex {}", env!("CARGO_PKG_VERSION"))
}

/// Build the spec for one installed game. Reads the install row and the launch
/// options; never scans compatibility tools (generation must not depend on what
/// this machine happens to have installed).
fn spec_for_game(
    app: &AppHandle,
    game_id: i64,
    args: Option<&str>,
) -> Result<(db::InstalledGame, PortableSpec), String> {
    let state = app.state::<AppState>();
    let (installed, options) = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        let installed = db::get_installed(&connection, game_id)?
            .ok_or_else(|| format!("Game {game_id} is not installed on this device."))?;
        let options = db::get_launch_options(&connection, game_id)?;
        (installed, options)
    };
    let spec = PortableSpec::from_install(
        &installed,
        &options,
        args.unwrap_or(""),
        &crate::util::now_rfc3339(),
        &generator_label(),
    )?;
    spec.validate()?;
    Ok((installed, spec))
}

/// Size and requirement of the bundle, for the dialog. Cheap enough to call on
/// every page visit: it walks the install folder once and reads one row.
pub fn estimate_for_game(app: &AppHandle, game_id: i64) -> Result<PortableEstimate, String> {
    let platform = std::env::consts::OS.to_string();
    if !supported() {
        return Ok(PortableEstimate {
            supported: false,
            platform,
            game_id,
            bytes: 0,
            runtime_requirement: RuntimeRequirement::LinuxNative.as_str().to_string(),
            executable_present: false,
            reason: Some("Portable Linux bundles can only be created on Linux.".to_string()),
        });
    }

    let (installed, spec) = match spec_for_game(app, game_id, None) {
        Ok(pair) => pair,
        Err(reason) => {
            return Ok(PortableEstimate {
                supported: true,
                platform,
                game_id,
                bytes: 0,
                runtime_requirement: RuntimeRequirement::LinuxNative.as_str().to_string(),
                executable_present: false,
                reason: Some(reason),
            });
        }
    };

    let root = PathBuf::from(&installed.install_path);
    let executable_present = root.join(&spec.executable).is_file();
    Ok(PortableEstimate {
        supported: true,
        platform,
        game_id,
        bytes: bundle_size(&root).saturating_add(estimated_script_bytes(&spec)),
        runtime_requirement: spec.runtime().as_str().to_string(),
        executable_present,
        reason: (!executable_present).then(|| {
            format!(
                "{} is not in the install folder any more. Verify the install first.",
                spec.executable
            )
        }),
    })
}

fn estimated_script_bytes(spec: &PortableSpec) -> u64 {
    (render_run_script(spec).len()
        + render_install_script(spec).len()
        + render_manifest(spec).len()
        + render_readme(spec).len()) as u64
}

/// Generate in place, or copy the folder to `destination` and generate there.
pub fn create_for_game(
    app: &AppHandle,
    game_id: i64,
    destination: Option<&str>,
    args: Option<&str>,
) -> Result<PortableBundleInfo, String> {
    if !supported() {
        return Err("Portable Linux bundles can only be created on Linux.".to_string());
    }
    let (installed, spec) = spec_for_game(app, game_id, args)?;
    let root = PathBuf::from(&installed.install_path);

    let destination = destination.map(str::trim).filter(|value| !value.is_empty());
    let Some(destination) = destination else {
        let bundle = generate(&root, &spec)?;
        emit_progress(
            app,
            game_id,
            "done",
            bundle.bytes,
            bundle.bytes,
            "Portable bundle written in the install folder",
        );
        return Ok(PortableBundleInfo {
            path: bundle.dir.to_string_lossy().to_string(),
            bytes: bundle.bytes,
            runtime_required: bundle.runtime.as_str().to_string(),
            file_count: bundle.files.len() as u32,
            replaced: bundle.replaced,
            copied: false,
        });
    };

    let destination = PathBuf::from(destination);
    if !destination.is_dir() {
        return Err(format!(
            "{} is not a folder. Pick an existing destination.",
            destination.display()
        ));
    }
    let normalized_root = paths::normalize_native_path(&root);
    let normalized_destination = paths::normalize_native_path(&destination);
    if normalized_destination == normalized_root || paths::is_inside(&root, &destination) {
        return Err("Choose a destination outside the install folder.".to_string());
    }
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| "The install folder has no name.".to_string())?;
    let target = destination.join(&name);
    if target.exists() {
        return Err(format!(
            "{} already exists. Remove it first, or pick another destination.",
            target.display()
        ));
    }

    let total = bundle_size(&root);
    emit_progress(app, game_id, "copying", 0, total, "Copying the game files");
    copy_tree(app, game_id, &root, &target, total)?;

    emit_progress(
        app,
        game_id,
        "generating",
        total,
        total,
        "Writing run.sh and install.sh",
    );
    let bundle = generate(&target, &spec)?;
    let bytes = bundle.bytes;
    emit_progress(
        app,
        game_id,
        "done",
        total,
        total,
        "Portable bundle created",
    );
    Ok(PortableBundleInfo {
        path: target.to_string_lossy().to_string(),
        bytes,
        runtime_required: bundle.runtime.as_str().to_string(),
        file_count: bundle.files.len() as u32,
        replaced: bundle.replaced,
        copied: true,
    })
}

fn emit_progress(
    app: &AppHandle,
    game_id: i64,
    phase: &str,
    bytes_done: u64,
    bytes_total: u64,
    message: &str,
) {
    let _ = app.emit(
        PROGRESS_EVENT,
        PortableProgress {
            game_id,
            phase: phase.to_string(),
            bytes_done,
            bytes_total,
            message: message.to_string(),
        },
    );
}

/// Copy the install folder, skipping the machine-specific prefixes. Symlinks are
/// skipped, matching the install copy rules: a bundle never carries a link that
/// escapes its own tree.
fn copy_tree(
    app: &AppHandle,
    game_id: i64,
    source: &Path,
    destination: &Path,
    total: u64,
) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("Could not create {}: {error}", destination.display()))?;
    let mut done = 0u64;
    let mut reported = 0u64;
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from_dir, to_dir)) = stack.pop() {
        let entries = std::fs::read_dir(&from_dir)
            .map_err(|error| format!("Could not read {}: {error}", from_dir.display()))?;
        for entry in entries.flatten() {
            let from = entry.path();
            let to = to_dir.join(entry.file_name());
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if is_skipped_dir(source, &from) {
                    tracing::info!(
                        path = %from.strip_prefix(source).unwrap_or(&from).display(),
                        "portable export skips a Wine prefix; install.sh builds a fresh one on the target"
                    );
                    continue;
                }
                std::fs::create_dir_all(&to)
                    .map_err(|error| format!("Could not create {}: {error}", to.display()))?;
                stack.push((from, to));
            } else if kind.is_file() {
                let size = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                std::fs::copy(&from, &to).map_err(|error| {
                    format!(
                        "Could not copy {} to {}: {error}",
                        from.display(),
                        to.display()
                    )
                })?;
                done = done.saturating_add(size);
                if done.saturating_sub(reported) >= PROGRESS_STEP || done >= total {
                    reported = done;
                    emit_progress(
                        app,
                        game_id,
                        "copying",
                        done,
                        total,
                        "Copying the game files",
                    );
                }
            }
        }
    }
    emit_progress(app, game_id, "copying", total, total, "Copy finished");
    Ok(())
}

/// #332: generate the bundle right after a successful Linux install.
///
/// Never fails the install: a missing executable (a repack that still needs its
/// setup) or an I/O error is logged and surfaced as a `portable://status` event.
/// Only folders Ludex installed itself are touched; an adopted folder belongs to
/// the user.
pub fn after_install(app: &AppHandle, game_id: i64, install_path: &Path) {
    if !supported() {
        return;
    }
    let source = {
        let state = app.state::<AppState>();
        let Ok(connection) = state.db.lock() else {
            return;
        };
        db::get_installed(&connection, game_id)
            .ok()
            .flatten()
            .map(|installed| installed.source)
    };
    if source.as_deref() != Some(db::SOURCE_LUDEX) {
        return;
    }
    let result =
        spec_for_game(app, game_id, None).and_then(|(_, spec)| generate(install_path, &spec));
    match result {
        Ok(bundle) => {
            tracing::info!(
                game_id,
                path = %bundle.dir.display(),
                runtime = bundle.runtime.as_str(),
                "portable bundle generated"
            );
            let _ = app.emit(
                STATUS_EVENT,
                PortableStatus {
                    game_id,
                    ok: true,
                    message: format!("Portable bundle ready ({})", bundle.runtime.as_str()),
                },
            );
        }
        Err(error) => {
            tracing::warn!(game_id, error = %error, "portable bundle not generated");
            let _ = app.emit(
                STATUS_EVENT,
                PortableStatus {
                    game_id,
                    ok: false,
                    message: error,
                },
            );
        }
    }
}

/// #332: size estimate for the UI. Runs the walk off the UI thread.
#[tauri::command]
pub async fn portable_bundle_estimate(
    app: AppHandle,
    game_id: i64,
) -> Result<PortableEstimate, String> {
    tauri::async_runtime::spawn_blocking(move || estimate_for_game(&app, game_id))
        .await
        .map_err(|error| format!("The portable bundle task failed: {error}"))?
}

/// #332: create the portable bundle in place, or copy the folder and create it
/// there. Progress arrives on `portable://progress`.
#[tauri::command]
pub async fn create_portable_bundle(
    app: AppHandle,
    game_id: i64,
    destination: Option<String>,
    args: Option<String>,
) -> Result<PortableBundleInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_for_game(&app, game_id, destination.as_deref(), args.as_deref())
    })
    .await
    .map_err(|error| format!("The portable bundle task failed: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::InstalledGame;
    use crate::linux_gaming::{DisplayProfile, LaunchMode};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-portable-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn native_spec() -> PortableSpec {
        PortableSpec {
            title: "Hades".to_string(),
            version: "1.0.38180".to_string(),
            slug: "hades".to_string(),
            executable: "Hades.sh".to_string(),
            working_dir: ".".to_string(),
            args: vec!["--windowed".to_string()],
            env: BTreeMap::from([("DXVK_HUD".to_string(), "fps".to_string())]),
            mode: PortableMode::Native,
            gameid: "1145360".to_string(),
            prefix: PREFIX_DIR.to_string(),
            script_version: SCRIPT_VERSION,
            generated_at: "2026-11-23T10:00:00Z".to_string(),
            generator: "Ludex 0.4.28".to_string(),
        }
    }

    fn windows_spec() -> PortableSpec {
        PortableSpec {
            title: "Outer Wilds".to_string(),
            version: "1.1.14".to_string(),
            slug: "outer-wilds".to_string(),
            executable: "bin/OuterWilds.exe".to_string(),
            working_dir: "bin".to_string(),
            args: vec!["--splash=false".to_string(), "a b".to_string()],
            mode: PortableMode::Windows,
            gameid: "0".to_string(),
            env: BTreeMap::new(),
            ..native_spec()
        }
    }

    /// The committed goldens; regenerating the scripts means updating them in the
    /// same commit.
    fn fixture_dir(kind: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/portable")
            .join(kind)
    }

    fn read_fixture(kind: &str, name: &str) -> String {
        std::fs::read_to_string(fixture_dir(kind).join(name))
            .unwrap_or_else(|error| panic!("{kind}/{name} is missing: {error}"))
    }

    /// Maintenance only: `cargo test --offline --lib portable::tests::write_goldens
    /// -- --ignored` rewrites the committed scripts after an intentional change.
    /// Never runs in CI, so a silent drift is impossible.
    #[test]
    #[ignore]
    fn write_goldens() {
        for (kind, spec) in [("native", native_spec()), ("windows", windows_spec())] {
            std::fs::create_dir_all(fixture_dir(kind)).unwrap();
            std::fs::write(fixture_dir(kind).join(RUN_SCRIPT), render_run_script(&spec)).unwrap();
            std::fs::write(
                fixture_dir(kind).join(INSTALL_SCRIPT),
                render_install_script(&spec),
            )
            .unwrap();
            std::fs::write(
                fixture_dir(kind).join(MANIFEST_FILE),
                format!("{}\n", render_manifest(&spec)),
            )
            .unwrap();
        }
    }

    #[test]
    fn native_scripts_match_the_committed_goldens() {
        let spec = native_spec();
        assert_eq!(
            render_run_script(&spec).trim_end(),
            read_fixture("native", RUN_SCRIPT).trim_end()
        );
        assert_eq!(
            render_install_script(&spec).trim_end(),
            read_fixture("native", INSTALL_SCRIPT).trim_end()
        );
        assert_eq!(
            render_manifest(&spec),
            read_fixture("native", MANIFEST_FILE).trim_end()
        );
    }

    #[test]
    fn windows_scripts_match_the_committed_goldens() {
        let spec = windows_spec();
        assert_eq!(
            render_run_script(&spec).trim_end(),
            read_fixture("windows", RUN_SCRIPT).trim_end()
        );
        assert_eq!(
            render_install_script(&spec).trim_end(),
            read_fixture("windows", INSTALL_SCRIPT).trim_end()
        );
        assert_eq!(
            render_manifest(&spec),
            read_fixture("windows", MANIFEST_FILE).trim_end()
        );
    }

    /// The whole point: no absolute path, no home, no machine name.
    #[test]
    fn scripts_never_reference_ludex_a_home_or_this_machine() {
        for spec in [native_spec(), windows_spec()] {
            for script in [render_run_script(&spec), render_install_script(&spec)] {
                assert!(!script.contains("/home/"), "a home leaked: {script}");
                assert!(
                    !script.contains(env!("CARGO_MANIFEST_DIR")),
                    "the build tree leaked: {script}"
                );
                assert!(
                    !script.contains(".local/share/opencode"),
                    "the worktree leaked: {script}"
                );
                // Every program is started relatively: `$SCRIPT_DIR`, `command
                // -v` or a bare name resolved from PATH. `$HOME` is only
                // *expanded* at run time, never baked in.
                assert!(
                    !script
                        .lines()
                        .any(|line| line.trim_start().starts_with("exec /")),
                    "an absolute program path was recorded: {script}"
                );
                assert!(
                    script.contains("SCRIPT_DIR"),
                    "paths must be re-rooted from the script folder"
                );
                // Advice is printed, never executed.
                assert!(
                    !script.lines().any(|line| line.starts_with("sudo ")),
                    "sudo must never run: {script}"
                );
            }
        }
    }

    #[test]
    fn manifests_and_readmes_carry_no_secret_and_no_absolute_path() {
        for spec in [native_spec(), windows_spec()] {
            let manifest = render_manifest(&spec);
            assert!(manifest.contains("\"kind\": \"ludex-portable-bundle\""));
            assert!(manifest.contains("\"scriptVersion\": 1"));
            assert!(manifest.contains(&format!(
                "\"runtimeRequirement\": \"{}\"",
                spec.runtime().as_str()
            )));
            assert!(!manifest.contains("/root"));
            assert!(!manifest.contains("/home/"));

            let readme = render_readme(&spec);
            assert!(readme.contains(&spec.title));
            assert!(readme.contains("./install.sh") && readme.contains("./run.sh"));
            assert!(readme.contains("HOW TO MOVE IT"));
            assert!(!readme.contains("/root/"));
            assert!(!readme.contains("/home/"));
        }
    }

    /// Spaces and unicode survive quoting, and `bash -n` accepts the result.
    #[test]
    fn spaces_and_unicode_are_quoted_and_parse() {
        let spec = PortableSpec {
            title: "Æther Blossom — Édition ½".to_string(),
            version: "2.0".to_string(),
            slug: "aether-blossom".to_string(),
            executable: "bin/Jeu épique.sh".to_string(),
            working_dir: "bin".to_string(),
            args: vec!["--name=Æther Blossom".to_string(), "it's here".to_string()],
            ..native_spec()
        };
        spec.validate().unwrap();
        let run = render_run_script(&spec);
        assert!(run.contains("LUDEX_EXECUTABLE='bin/Jeu épique.sh'"));
        assert!(run.contains("LUDEX_ARGS=('--name=Æther Blossom' 'it'\\''s here')"));
        assert_bash_parses(&run);
        assert_bash_parses(&render_install_script(&spec));
    }

    fn bash() -> Option<&'static str> {
        ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .into_iter()
            .find(|candidate| Path::new(candidate).is_file())
    }

    /// A PATH with only the tools the scripts need, so a runtime installed on
    /// the build machine cannot change the outcome.
    #[cfg(unix)]
    fn minimal_path(dir: &Path) {
        for tool in [
            "chmod",
            "grep",
            "find",
            "sort",
            "tail",
            "dirname",
            "readlink",
            "mkdir",
            "sha256sum",
        ] {
            if let Some(path) = crate::paths::find_on_path(tool) {
                let _ = std::os::unix::fs::symlink(path, dir.join(tool));
            }
        }
    }

    fn assert_bash_parses(script: &str) {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("parse");
        let path = dir.join("script.sh");
        std::fs::write(&path, script).unwrap();
        let status = std::process::Command::new(bash)
            .arg("-n")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "bash -n refused the generated script");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_specs_are_rejected() {
        let mut spec = native_spec();
        spec.title = "bad\nname".to_string();
        assert!(spec.validate().is_err());

        let mut spec = native_spec();
        spec.executable = "../escape.sh".to_string();
        assert!(spec.validate().unwrap_err().contains(".."));

        let mut spec = native_spec();
        spec.executable = "/usr/bin/game".to_string();
        assert!(spec.validate().is_err());

        let mut spec = native_spec();
        spec.args = vec!["line\nbreak".to_string()];
        assert!(spec.validate().is_err());

        let mut spec = native_spec();
        spec.slug = "Not A Slug".to_string();
        assert!(spec.validate().is_err());

        let mut spec = native_spec();
        spec.env
            .insert("LUDEX_MODE".to_string(), "native".to_string());
        assert!(spec.validate().unwrap_err().contains("LUDEX_"));

        let mut spec = native_spec();
        spec.env
            .insert("WINEPREFIX".to_string(), "/home/someone/prefix".to_string());
        assert!(spec.validate().unwrap_err().contains("WINEPREFIX"));
    }

    #[test]
    fn generation_replaces_archive_planted_files() {
        let dir = temp_dir("replace");
        std::fs::write(dir.join(RUN_SCRIPT), "#!/bin/sh\nrm -rf /\n").unwrap();
        std::fs::write(dir.join(INSTALL_SCRIPT), "junk").unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), "{}").unwrap();
        let bundle = generate(&dir, &native_spec()).unwrap();
        assert_eq!(
            bundle.replaced,
            vec![
                RUN_SCRIPT.to_string(),
                INSTALL_SCRIPT.to_string(),
                MANIFEST_FILE.to_string()
            ]
        );
        let run = std::fs::read_to_string(dir.join(RUN_SCRIPT)).unwrap();
        assert!(run.starts_with("#!/usr/bin/env bash"));
        assert!(!run.contains("rm -rf"));
        assert!(dir.join(README_FILE).is_file());
        assert!(
            !dir.join(PREFIX_DIR).exists(),
            "the prefix is created lazily"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Run a generated native bundle from another path, with a clean HOME.
    #[test]
    #[cfg(unix)]
    fn a_moved_native_bundle_runs_with_a_clean_home() {
        let Some(bash) = bash() else {
            return;
        };
        let source = temp_dir("move source");
        let bundle_dir = source.join("Hades");
        std::fs::create_dir_all(&bundle_dir).unwrap();
        let game = bundle_dir.join("Hades.sh");
        std::fs::write(
            &game,
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s|%s|%s\\n' \"$(pwd)\" \"$HOME\" \"$*\" >> \"$LUDEX_MARKER\"\n",
        )
        .unwrap();
        let spec = PortableSpec {
            executable: "Hades.sh".to_string(),
            args: vec!["recorded arg".to_string()],
            ..native_spec()
        };
        generate(&bundle_dir, &spec).unwrap();

        let moved = temp_dir("moved target with spaces é");
        let moved_bundle = moved.join("Hades é");
        std::fs::rename(&bundle_dir, &moved_bundle).unwrap();
        std::fs::remove_dir_all(&source).ok();

        let home = temp_dir("clean home");
        let marker = moved.join("marker.txt");
        let status = std::process::Command::new(bash)
            .arg(moved_bundle.join(RUN_SCRIPT))
            .arg("extra arg")
            .env("HOME", &home)
            .env("LUDEX_MARKER", &marker)
            .env_remove("LD_LIBRARY_PATH")
            .status()
            .expect("bash starts");
        assert!(status.success(), "the moved bundle ran");
        let line = std::fs::read_to_string(&marker).unwrap();
        assert!(
            line.starts_with(&format!("{}|", moved_bundle.display())),
            "the working directory is the bundle: {line}"
        );
        assert!(line.contains(&format!("|{}|", home.display())));
        assert!(line.contains("recorded arg extra arg"));
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&moved).ok();
    }

    /// `install.sh` is idempotent and fixes the executable bits.
    #[test]
    #[cfg(unix)]
    fn install_sh_is_idempotent_for_a_native_bundle() {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("idempotent");
        let game = dir.join("Hades.sh");
        std::fs::write(&game, "#!/usr/bin/env bash\nexit 0\n").unwrap();
        generate(&dir, &native_spec()).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&game, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        for attempt in 0..2 {
            let output = std::process::Command::new(bash)
                .arg(dir.join(INSTALL_SCRIPT))
                .env("HOME", &dir)
                .output()
                .expect("bash starts");
            assert!(
                output.status.success(),
                "install.sh attempt {attempt} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.contains("Scripts and game file: executable"));
            assert!(text.contains("Native Linux build"));
            assert!(text.contains("Done. Start the game with: ./run.sh"));
            assert!(dir.join(PREFIX_DIR).is_dir());
        }
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&game).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "the game binary is executable");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A Windows bundle with a runtime inside `support/runtime`: precedence,
    /// environment and argument quoting, without UMU or Wine installed.
    #[test]
    #[cfg(unix)]
    fn a_bundled_windows_runtime_wins_and_receives_the_planned_environment() {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("bundled runtime");
        let spec = PortableSpec {
            executable: "bin/OuterWilds.exe".to_string(),
            working_dir: ".".to_string(),
            args: vec!["--splash=false".to_string()],
            env: BTreeMap::from([("DXVK_HUD".to_string(), "fps".to_string())]),
            ..windows_spec()
        };
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/OuterWilds.exe"), b"MZ").unwrap();
        let runtime = dir.join("support/runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("proton"), "#!/usr/bin/env bash\n").unwrap();
        std::fs::write(
            runtime.join("umu-run"),
            "#!/usr/bin/env bash\nset -euo pipefail\nshift\nprintf '%s\\n' \"PROTONPATH=$PROTONPATH\" \"WINEPREFIX=$WINEPREFIX\" \"GAMEID=$GAMEID\" \"DXVK_HUD=$DXVK_HUD\" \"ARGS=$*\" > \"$LUDEX_MARKER\"\n",
        )
        .unwrap();
        generate(&dir, &spec).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                runtime.join("umu-run"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let home = temp_dir("windows home");
        let marker = dir.join("runtime.txt");
        let output = std::process::Command::new(bash)
            .arg(dir.join(RUN_SCRIPT))
            .arg("extra")
            .env("HOME", &home)
            .env("LUDEX_MARKER", &marker)
            .output()
            .expect("bash starts");
        assert!(
            output.status.success(),
            "run.sh failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = std::fs::read_to_string(&marker).unwrap();
        assert!(
            text.contains(&format!("PROTONPATH={}", runtime.display())),
            "the bundled Proton build is used: {text}"
        );
        assert!(
            text.contains(&format!("WINEPREFIX={}", dir.join(PREFIX_DIR).display())),
            "{text}"
        );
        assert!(text.contains("GAMEID=0"), "{text}");
        assert!(text.contains("DXVK_HUD=fps"), "{text}");
        assert!(text.contains("ARGS=--splash=false extra"), "{text}");
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A bundle that ships only a Wine build still starts: the bundled runtime
    /// is tried before anything installed on the machine.
    #[test]
    #[cfg(unix)]
    fn a_bundled_wine_only_runtime_is_used() {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("bundled wine");
        let spec = PortableSpec {
            executable: "bin/OuterWilds.exe".to_string(),
            working_dir: ".".to_string(),
            ..windows_spec()
        };
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/OuterWilds.exe"), b"MZ").unwrap();
        let runtime_bin = dir.join("support/runtime/bin");
        std::fs::create_dir_all(&runtime_bin).unwrap();
        std::fs::write(
            runtime_bin.join("wine"),
            "#!/usr/bin/env bash\nset -euo pipefail\nshift\nprintf '%s|%s\\n' \"$WINEPREFIX\" \"$*\" > \"$LUDEX_MARKER\"\n",
        )
        .unwrap();
        generate(&dir, &spec).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                runtime_bin.join("wine"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        let home = temp_dir("wine home");
        let marker = dir.join("wine.txt");
        let output = std::process::Command::new(bash)
            .arg(dir.join(RUN_SCRIPT))
            .env("HOME", &home)
            .env("LUDEX_MARKER", &marker)
            .output()
            .expect("bash starts");
        assert!(
            output.status.success(),
            "run.sh failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = std::fs::read_to_string(&marker).unwrap();
        assert!(text.starts_with(&format!("{}|", dir.join(PREFIX_DIR).display())));
        assert!(text.contains("--splash=false a b"));
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Missing runtime: the message names the packages and the exit code is
    /// non-zero. A curated PATH guarantees no runtime is visible, whatever the
    /// build machine has installed.
    #[test]
    #[cfg(unix)]
    fn a_windows_bundle_without_a_runtime_fails_with_actionable_advice() {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("missing runtime");
        let spec = PortableSpec {
            executable: "bin/OuterWilds.exe".to_string(),
            working_dir: ".".to_string(),
            ..windows_spec()
        };
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/OuterWilds.exe"), b"MZ").unwrap();
        generate(&dir, &spec).unwrap();

        let bin = temp_dir("minimal bin");
        minimal_path(&bin);
        let home = temp_dir("empty home");

        for script in [INSTALL_SCRIPT, RUN_SCRIPT] {
            let output = std::process::Command::new(bash)
                .arg(dir.join(script))
                .env("HOME", &home)
                .env("PATH", &bin)
                .env_remove("PROTONPATH")
                .output()
                .expect("bash starts");
            assert!(
                !output.status.success(),
                "{script} must not claim success without a runtime"
            );
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(text.contains("cannot run it yet"), "{script}: {text}");
            assert!(text.contains("pacman -S umu-launcher"), "{script}: {text}");
            assert!(text.contains("dnf install wine"), "{script}: {text}");
            assert!(text.contains("apt install wine"), "{script}: {text}");
            assert!(
                text.contains("Nothing was installed and nothing was started"),
                "{script}: {text}"
            );
            assert!(!text.contains("Done."), "{script} claimed success: {text}");
        }
        std::fs::remove_dir_all(&bin).ok();
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `install.sh` initialises the prefix once, through the bundled Wine, and
    /// the second run does nothing.
    #[test]
    #[cfg(unix)]
    fn install_sh_initialises_the_prefix_once() {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("prefix init");
        let spec = PortableSpec {
            executable: "bin/OuterWilds.exe".to_string(),
            working_dir: ".".to_string(),
            ..windows_spec()
        };
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/OuterWilds.exe"), b"MZ").unwrap();
        let runtime = dir.join("support/runtime");
        std::fs::create_dir_all(runtime.join("bin")).unwrap();
        std::fs::write(runtime.join("proton"), "#!/usr/bin/env bash\n").unwrap();
        let calls = dir.join("wineboot-calls.txt");
        std::fs::write(
            runtime.join("bin/wine"),
            "#!/usr/bin/env bash\nprintf '%s|%s\\n' \"$WINEPREFIX\" \"$*\" >> \"$LUDEX_CALLS\"\n",
        )
        .unwrap();
        generate(&dir, &spec).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                runtime.join("bin/wine"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        for attempt in 0..2 {
            let output = std::process::Command::new(bash)
                .arg(dir.join(INSTALL_SCRIPT))
                .env("HOME", &dir)
                .env("LUDEX_CALLS", &calls)
                .output()
                .expect("bash starts");
            assert!(
                output.status.success(),
                "install.sh attempt {attempt} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let calls = std::fs::read_to_string(&calls).unwrap();
        assert_eq!(calls.lines().count(), 1, "wineboot ran once");
        assert!(calls.contains(&format!("{}|wineboot -u", dir.join(PREFIX_DIR).display())));
        assert!(dir.join(PREFIX_DIR).join(".ludex-initialised").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `run.sh` refuses a manifest that is missing or names another game.
    #[test]
    #[cfg(unix)]
    fn run_sh_refuses_a_missing_or_foreign_manifest() {
        let Some(bash) = bash() else {
            return;
        };
        let dir = temp_dir("foreign manifest");
        std::fs::write(dir.join("Hades.sh"), "#!/usr/bin/env bash\n").unwrap();
        generate(&dir, &native_spec()).unwrap();
        let home = temp_dir("manifest home");

        let good = std::process::Command::new(bash)
            .arg(dir.join(RUN_SCRIPT))
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(
            good.status.success(),
            "the intact bundle starts: {}",
            String::from_utf8_lossy(&good.stderr)
        );

        std::fs::remove_file(dir.join(MANIFEST_FILE)).unwrap();
        let absent = std::process::Command::new(bash)
            .arg(dir.join(RUN_SCRIPT))
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(!absent.status.success());
        assert!(String::from_utf8_lossy(&absent.stderr).contains("ludex-game.json is missing"));

        let mut other = native_spec();
        other.slug = "celeste".to_string();
        std::fs::write(dir.join(MANIFEST_FILE), render_manifest(&other)).unwrap();
        let foreign = std::process::Command::new(bash)
            .arg(dir.join(RUN_SCRIPT))
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(!foreign.status.success());
        assert!(
            String::from_utf8_lossy(&foreign.stderr).contains("describes another game"),
            "{}",
            String::from_utf8_lossy(&foreign.stderr)
        );
        std::fs::remove_dir_all(&home).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The install row drives the spec: relative executable, working folder,
    /// GAMEID and mode.
    #[test]
    fn install_rows_become_relative_specs() {
        let root = temp_dir("install row");
        let installed = InstalledGame {
            game_id: 7,
            version_id: 9,
            title: "Outer Wilds".to_string(),
            version: Some("1.1.14".to_string()),
            install_path: root.to_string_lossy().to_string(),
            executable: Some(
                root.join("bin/OuterWilds.exe")
                    .to_string_lossy()
                    .to_string(),
            ),
            size_bytes: 0,
            installed_at: "2026-11-23T10:00:00Z".to_string(),
            last_launched_at: None,
            archive_hash: None,
            install_root: Some(root.to_string_lossy().to_string()),
            source: db::SOURCE_LUDEX.to_string(),
            launch_uri: None,
        };
        let options = LaunchOptions {
            mode: LaunchMode::Umu,
            steam_app_id: Some(753640),
            working_dir: root.join("bin").to_string_lossy().to_string(),
            env: BTreeMap::from([("DXVK_HUD".to_string(), "fps".to_string())]),
            display: DisplayProfile::default(),
            ..LaunchOptions::default()
        };
        let spec = PortableSpec::from_install(
            &installed,
            &options,
            "--x  y",
            "2026-11-23T10:00:00Z",
            "Ludex 0.4.28",
        )
        .unwrap();
        assert_eq!(spec.slug, "outer-wilds");
        assert_eq!(spec.executable, "bin/OuterWilds.exe");
        assert_eq!(spec.working_dir, "bin");
        assert_eq!(spec.mode, PortableMode::Windows);
        assert_eq!(spec.runtime(), RuntimeRequirement::WindowsCompat);
        assert_eq!(spec.gameid, "753640");
        assert_eq!(spec.args, vec!["--x".to_string(), "y".to_string()]);

        // A native binary at the root keeps the folder as working directory.
        let native = InstalledGame {
            executable: Some(root.join("Celeste.sh").to_string_lossy().to_string()),
            ..installed
        };
        let spec = PortableSpec::from_install(
            &native,
            &LaunchOptions::default(),
            "",
            "2026-11-23T10:00:00Z",
            "Ludex 0.4.28",
        )
        .unwrap();
        assert_eq!(spec.mode, PortableMode::Native);
        assert_eq!(spec.working_dir, ".");
        assert_eq!(spec.runtime(), RuntimeRequirement::LinuxNative);
        assert_eq!(spec.gameid, "0");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_size_estimate_skips_both_prefixes() {
        let root = temp_dir("size");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/game.dat"), vec![1u8; 1000]).unwrap();
        std::fs::create_dir_all(root.join(".prefix/drive_c")).unwrap();
        std::fs::write(root.join(".prefix/drive_c/huge"), vec![1u8; 5000]).unwrap();
        std::fs::create_dir_all(root.join(PREFIX_DIR)).unwrap();
        std::fs::write(root.join(PREFIX_DIR).join("system.reg"), vec![1u8; 7000]).unwrap();
        assert_eq!(bundle_size(&root), 1000);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn shell_quoting_is_unambiguous() {
        assert_eq!(shell_quote("plain"), "plain");
        assert_eq!(shell_quote("with space"), "'with space'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a$b"), "'a$b'");
    }
}
