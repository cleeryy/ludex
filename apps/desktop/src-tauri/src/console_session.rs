//! Console session (#286): a Wayland session entry and the default boot target.
//!
//! Console mode was a window. A console is what a machine boots into. This
//! module ships `ludex-console.desktop` (a Wayland session that runs
//! `ludex --console`) and the autologin configuration each supported display
//! manager needs to start it without a login prompt:
//!
//! - SDDM writes `/etc/sddm.conf.d/90-ludex-console.conf`;
//! - GDM writes `/etc/gdm/custom.conf` (or `/etc/gdm3/custom.conf` on Debian)
//!   and the AccountsService `Session=` key GDM actually reads;
//! - greetd writes `[initial_session]` in `/etc/greetd/config.toml`;
//! - LightDM writes `/etc/lightdm/lightdm.conf.d/50-ludex-console.conf`.
//!
//! Safety rules encoded here, because these are system files:
//!
//! - Every file is read before it is written and backed up under
//!   `/var/lib/ludex/console-session/backups/`; a revert restores the exact
//!   bytes, and deletes the files Ludex created.
//! - Generation is pure and data-driven ([`plan_enable`], [`plan_disable`]);
//!   the dry-run returns the files, their contents and the exact commands
//!   without touching anything.
//! - No display manager means no toggle: the plan is refused with a sentence
//!   naming what was looked for, never a guess.
//! - The elevated script only contains `install` and `rm` with quoted absolute
//!   paths; contents are staged in a private temporary directory, so no shell
//!   interpolation ever sees a config value.
//!
//! Reverting is path-validated against the display manager's known files, so a
//! tampered state file cannot turn the restore into an arbitrary file write.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::window_policy::Platform;

/* -------------------------------- constants ------------------------------- */

/// Basename of the session entry, as the display manager lists it.
pub const SESSION_ID: &str = "ludex-console";
/// Filename under `wayland-sessions/`.
pub const SESSION_ENTRY_NAME: &str = "ludex-console.desktop";
/// The shipped entry. Embedded so an AppImage install needs no repository.
pub const SESSION_ENTRY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../packaging/sessions/ludex-console.desktop"
));
/// The command the session entry runs.
pub const CONSOLE_COMMAND: &str = "ludex --console";
/// SDDM drop-in, read after the main configuration.
pub const SDDM_CONF: &str = "sddm.conf.d/90-ludex-console.conf";
/// GDM (Fedora, Arch, openSUSE) daemon configuration.
pub const GDM_CONF: &str = "gdm/custom.conf";
/// GDM on Debian and Ubuntu.
pub const GDM3_CONF: &str = "gdm3/custom.conf";
/// greetd configuration.
pub const GREETD_CONF: &str = "greetd/config.toml";
/// LightDM drop-in under `lightdm.conf.d`.
pub const LIGHTDM_CONF: &str = "lightdm/lightdm.conf.d/50-ludex-console.conf";
/// Root-owned record of what Ludex changed, under `/var`.
const STATE_DIR: &str = "lib/ludex/console-session";
/// Largest file this module will restore. Display manager configuration is
/// small; anything bigger means the state file was tampered with.
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/* --------------------------------- paths ---------------------------------- */

/// The three system roots every path is derived from. Tests point them at a
/// temporary directory so nothing here ever touches the real `/etc`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolePaths {
    pub etc: PathBuf,
    pub var: PathBuf,
    pub share: PathBuf,
}

impl ConsolePaths {
    pub fn system() -> Self {
        Self {
            etc: PathBuf::from("/etc"),
            var: PathBuf::from("/var"),
            share: PathBuf::from("/usr/share"),
        }
    }

    /// Entry point for tests: the same layout under a scratch root.
    #[cfg(test)]
    pub fn under(root: &Path) -> Self {
        Self {
            etc: root.join("etc"),
            var: root.join("var"),
            share: root.join("usr/share"),
        }
    }

    pub fn entry(&self) -> PathBuf {
        self.share.join("wayland-sessions").join(SESSION_ENTRY_NAME)
    }

    pub fn state_dir(&self) -> PathBuf {
        self.var.join(STATE_DIR)
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir().join("state.json")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.state_dir().join("backups")
    }
}

/// Temporary staging directory for one elevated request. The directory is
/// created `0700` and removed after the command; the path is deterministic for
/// the process so the dry-run shows the exact commands it would run.
pub fn staging_dir() -> PathBuf {
    std::env::temp_dir().join(format!("ludex-console-session-{}", std::process::id()))
}

/* --------------------------------- models --------------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionManager {
    Sddm,
    Gdm,
    Greetd,
    Lightdm,
}

impl SessionManager {
    pub fn id(self) -> &'static str {
        match self {
            Self::Sddm => "sddm",
            Self::Gdm => "gdm",
            Self::Greetd => "greetd",
            Self::Lightdm => "lightdm",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sddm => "SDDM",
            Self::Gdm => "GDM",
            Self::Greetd => "greetd",
            Self::Lightdm => "LightDM",
        }
    }
}

/// What the UI shows about the detected manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerInfo {
    pub id: String,
    pub name: String,
    /// Files the toggle will write for this manager.
    pub config_paths: Vec<String>,
}

/// The probe's input. Kept separate from the running system so every detection
/// branch is testable on any host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagerProbe {
    /// Target of `/etc/systemd/system/display-manager.service` (the active one).
    pub display_manager_target: Option<String>,
    /// `/etc/X11/default-display-manager` (Debian's own marker).
    pub default_display_manager: Option<String>,
    /// Installed greeter binaries/services found on PATH.
    pub binaries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detection {
    pub manager: Option<SessionManager>,
    /// Whether GDM's Debian layout (`/etc/gdm3`) is in use.
    pub gdm3: bool,
    /// Human sentence when no manager was found or the choice is ambiguous.
    pub reason: String,
    /// What the probe saw, for the diagnostics bundle and the UI.
    pub evidence: Vec<String>,
}

/// Map a path, unit name or binary name to a manager. Only the four supported
/// managers are recognised.
pub fn manager_from_hint(hint: &str) -> Option<SessionManager> {
    let lower = hint.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    if lower.contains("sddm") {
        Some(SessionManager::Sddm)
    } else if lower.contains("lightdm") {
        Some(SessionManager::Lightdm)
    } else if lower.contains("greetd") || lower.contains("regreet") {
        Some(SessionManager::Greetd)
    } else if lower.contains("gdm") {
        Some(SessionManager::Gdm)
    } else {
        None
    }
}

/// Resolve a probe. A hint from systemd or Debian wins; otherwise exactly one
/// recognised manager must be installed. Two installed managers are ambiguous
/// and refused instead of guessed.
pub fn detect_from(probe: &ManagerProbe, gdm3: bool) -> Detection {
    let mut evidence = Vec::new();
    if let Some(target) = probe
        .display_manager_target
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        evidence.push(format!("display-manager.service → {target}"));
        if let Some(manager) = manager_from_hint(target) {
            return Detection {
                manager: Some(manager),
                gdm3,
                reason: String::new(),
                evidence,
            };
        }
    }
    if let Some(marker) = probe
        .default_display_manager
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        evidence.push(format!("/etc/X11/default-display-manager → {marker}"));
        if let Some(manager) = manager_from_hint(marker) {
            return Detection {
                manager: Some(manager),
                gdm3,
                reason: String::new(),
                evidence,
            };
        }
    }

    let mut found: BTreeSet<SessionManager> = BTreeSet::new();
    for binary in &probe.binaries {
        if let Some(manager) = manager_from_hint(binary) {
            found.insert(manager);
        }
    }
    if !found.is_empty() {
        evidence.push(format!(
            "installed session manager(s): {}",
            found
                .iter()
                .map(|manager| manager.name())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match found.len() {
        0 => Detection {
            manager: None,
            gdm3,
            reason: "Ludex found no display manager (SDDM, GDM, LightDM or greetd) on this machine, so it cannot make the console session the boot target.".to_string(),
            evidence,
        },
        1 => Detection {
            manager: found.into_iter().next(),
            gdm3,
            reason: String::new(),
            evidence,
        },
        _ => Detection {
            manager: None,
            gdm3,
            reason: format!(
                "Several display managers are installed ({}), and none is marked as the active one. Set the default display manager first; Ludex will not guess.",
                found
                    .iter()
                    .map(|manager| manager.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            evidence,
        },
    }
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|directory| directory.join(name).is_file()))
        .unwrap_or(false)
}

/// Probe the running machine. Every read is unprivileged and best-effort.
pub fn probe_managers(paths: &ConsolePaths) -> Detection {
    if crate::window_policy::current_platform() != Platform::Linux {
        return Detection {
            manager: None,
            gdm3: false,
            reason:
                "Booting straight into console mode is only implemented for Linux display managers."
                    .to_string(),
            evidence: Vec::new(),
        };
    }
    let link = paths.etc.join("systemd/system/display-manager.service");
    let display_manager_target = std::fs::read_link(&link)
        .ok()
        .map(|target| target.to_string_lossy().to_string());
    let default_display_manager =
        std::fs::read_to_string(paths.etc.join("X11/default-display-manager"))
            .ok()
            .map(|value| value.trim().to_string());
    let mut binaries = Vec::new();
    for name in ["sddm", "gdm", "gdm3", "lightdm", "greetd", "regreet"] {
        if on_path(name) {
            binaries.push(name.to_string());
        }
    }
    let gdm3 = paths.etc.join("gdm3").is_dir();
    detect_from(
        &ManagerProbe {
            display_manager_target,
            default_display_manager,
            binaries,
        },
        gdm3,
    )
}

/* --------------------------------- errors --------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleSessionError {
    pub code: String,
    pub message: String,
}

impl ConsoleSessionError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ConsoleSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ConsoleSessionError {}

pub const CODE_UNSUPPORTED: &str = "unsupported";
pub const CODE_NO_MANAGER: &str = "noSessionManager";
pub const CODE_CONSENT: &str = "consentRequired";
pub const CODE_NOTHING_TO_REVERT: &str = "nothingToRevert";
pub const CODE_ALREADY_ENABLED: &str = "alreadyEnabled";
pub const CODE_ELEVATION_UNAVAILABLE: &str = "elevationUnavailable";
pub const CODE_ELEVATION_CANCELLED: &str = "elevationCancelled";
pub const CODE_ELEVATION_FAILED: &str = "elevationFailed";
pub const CODE_ENTRY_CONFLICT: &str = "entryConflict";
pub const CODE_BACKUP_MISSING: &str = "backupMissing";
pub const CODE_STATE_INVALID: &str = "stateInvalid";
pub const CODE_INVALID_USER: &str = "invalidUser";
pub const CODE_FAILED: &str = "failed";

/* -------------------------------- file plan ------------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlannedAction {
    Write,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub path: String,
    pub action: PlannedAction,
    /// UTF-8 view for the consent dialog. Display manager configuration is
    /// text; the exact bytes used by apply live in `bytes`.
    pub contents: String,
    /// Whether the target existed before Ludex touched it.
    pub existed: bool,
    /// Backup path, only when the original existed.
    pub backup: Option<String>,
    pub mode: u32,
    pub description: String,
    #[serde(skip)]
    pub bytes: Vec<u8>,
    #[serde(skip)]
    pub original: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleSessionPlan {
    pub enable: bool,
    pub manager: Option<ManagerInfo>,
    pub user: Option<String>,
    pub session: String,
    pub files: Vec<PlannedFile>,
    pub state_file: String,
    pub commands: Vec<String>,
    pub warnings: Vec<String>,
    pub message: String,
    #[serde(skip)]
    pub state_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedFile {
    pub path: String,
    pub backup: Option<String>,
    pub existed: bool,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedState {
    pub manager: SessionManager,
    pub gdm3: bool,
    pub user: String,
    pub enabled: bool,
    pub applied_at: String,
    pub files: Vec<AppliedFile>,
}

/* ---------------------------- config generation --------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    SessionEntry,
    SddmAutologin,
    GdmAutologin,
    GdmAccountsSession,
    GreetdInitialSession,
    LightdmAutologin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetSpec {
    path: PathBuf,
    mode: u32,
    description: &'static str,
    kind: TargetKind,
}

fn entry_spec(paths: &ConsolePaths) -> TargetSpec {
    TargetSpec {
        path: paths.entry(),
        mode: 0o644,
        description: "Ludex Console session entry",
        kind: TargetKind::SessionEntry,
    }
}

/// The files one display manager needs. Data-driven: adding a manager means
/// adding a row here and a merge arm in [`render`].
fn dm_targets(
    manager: SessionManager,
    paths: &ConsolePaths,
    gdm3: bool,
    user: &str,
) -> Vec<TargetSpec> {
    match manager {
        SessionManager::Sddm => vec![TargetSpec {
            path: paths.etc.join(SDDM_CONF),
            mode: 0o644,
            description: "SDDM autologin drop-in",
            kind: TargetKind::SddmAutologin,
        }],
        SessionManager::Gdm => {
            let custom = if gdm3 { GDM3_CONF } else { GDM_CONF };
            vec![
                TargetSpec {
                    path: paths.etc.join(custom),
                    mode: 0o644,
                    description: "GDM autologin daemon configuration",
                    kind: TargetKind::GdmAutologin,
                },
                TargetSpec {
                    path: paths.var.join("lib/AccountsService/users").join(user),
                    mode: 0o644,
                    description: "AccountsService session GDM logs in with",
                    kind: TargetKind::GdmAccountsSession,
                },
            ]
        }
        SessionManager::Greetd => vec![TargetSpec {
            path: paths.etc.join(GREETD_CONF),
            mode: 0o644,
            description: "greetd initial session",
            kind: TargetKind::GreetdInitialSession,
        }],
        SessionManager::Lightdm => vec![TargetSpec {
            path: paths.etc.join(LIGHTDM_CONF),
            mode: 0o644,
            description: "LightDM autologin drop-in",
            kind: TargetKind::LightdmAutologin,
        }],
    }
}

/// Replace or append keys inside one INI section, preserving every other line
/// (comments, unknown keys, other sections) so a revert-by-backup is the only
/// thing that can lose data.
pub fn merge_ini(existing: Option<&str>, section: &str, pairs: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = existing
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let header = format!("[{section}]");
    match lines.iter().position(|line| line.trim() == header) {
        Some(start) => {
            let end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find(|(_, line)| line.trim_start().starts_with('['))
                .map(|(index, _)| index)
                .unwrap_or(lines.len());
            let mut remaining: Vec<(&str, &str)> = pairs.to_vec();
            let mut replaced: BTreeSet<String> = BTreeSet::new();
            for line in lines.iter_mut().take(end).skip(start + 1) {
                let trimmed = line.trim_start().to_string();
                let Some(equals) = trimmed.find('=') else {
                    continue;
                };
                let key = trimmed[..equals].trim();
                if let Some(position) = remaining.iter().position(|(name, _)| *name == key) {
                    let (name, value) = remaining.remove(position);
                    *line = format!("{name}={value}");
                    replaced.insert(name.to_string());
                } else if replaced.contains(key) {
                    // A duplicate would be the last value an INI parser reads
                    // and would undo the line we just set; drop it instead.
                    line.clear();
                }
            }
            for (offset, (name, value)) in remaining.into_iter().enumerate() {
                lines.insert(end + offset, format!("{name}={value}"));
            }
        }
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            for (name, value) in pairs {
                lines.push(format!("{name}={value}"));
            }
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Replace or append a TOML table. greetd's `[initial_session]` is the one
/// documented autologin hook, so the body is regenerated and every other table
/// is left alone.
pub fn merge_toml_table(existing: Option<&str>, table: &str, pairs: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = existing
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let header = format!("[{table}]");
    let replacement: Vec<String> = pairs
        .iter()
        .map(|(name, value)| format!("{name} = \"{}\"", toml_string(value)))
        .collect();
    match lines.iter().position(|line| line.trim() == header) {
        Some(start) => {
            let end = lines
                .iter()
                .enumerate()
                .skip(start + 1)
                .find(|(_, line)| line.trim_start().starts_with('['))
                .map(|(index, _)| index)
                .unwrap_or(lines.len());
            lines.splice(start + 1..end, replacement);
        }
        None => {
            if lines.last().is_some_and(|line| !line.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.extend(replacement);
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn render(kind: TargetKind, existing: Option<&str>, user: &str) -> String {
    match kind {
        TargetKind::SessionEntry => SESSION_ENTRY.to_string(),
        TargetKind::SddmAutologin => merge_ini(
            existing,
            "Autologin",
            &[("User", user), ("Session", SESSION_ID), ("Relogin", "true")],
        ),
        TargetKind::GdmAutologin => merge_ini(
            existing,
            "daemon",
            &[
                ("AutomaticLoginEnable", "true"),
                ("AutomaticLogin", user),
                ("WaylandEnable", "true"),
            ],
        ),
        TargetKind::GdmAccountsSession => merge_ini(existing, "User", &[("Session", SESSION_ID)]),
        TargetKind::GreetdInitialSession => merge_toml_table(
            existing,
            "initial_session",
            &[("command", CONSOLE_COMMAND), ("user", user)],
        ),
        TargetKind::LightdmAutologin => merge_ini(
            existing,
            "Seat:*",
            &[
                ("autologin-user", user),
                ("autologin-user-timeout", "0"),
                ("autologin-session", SESSION_ID),
                ("user-session", SESSION_ID),
            ],
        ),
    }
}

/* ------------------------------ filesystem -------------------------------- */

/// The three operations a plan needs. Production plans use a read-only
/// implementation; apply tests use an in-memory one; the elevated apply stages
/// each write and records the exact `install`/`rm` command.
pub trait PlanFs {
    fn read(&mut self, path: &str) -> io::Result<Option<Vec<u8>>>;
    fn write(&mut self, slot: &str, path: &str, bytes: &[u8], mode: u32) -> io::Result<()>;
    fn remove(&mut self, path: &str) -> io::Result<()>;
}

/// Reads only; used to build a plan. Writes are done by the elevated helper.
pub struct ReadOnlyFs;

impl PlanFs for ReadOnlyFs {
    fn read(&mut self, path: &str) -> io::Result<Option<Vec<u8>>> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write(&mut self, _slot: &str, _path: &str, _bytes: &[u8], _mode: u32) -> io::Result<()> {
        Err(io::Error::other(
            "the read-only plan builder cannot write files",
        ))
    }

    fn remove(&mut self, _path: &str) -> io::Result<()> {
        Err(io::Error::other(
            "the read-only plan builder cannot remove files",
        ))
    }
}

fn filesystem_error(path: &str, error: io::Error) -> ConsoleSessionError {
    ConsoleSessionError::new(CODE_FAILED, format!("Could not access {path}: {error}"))
}

/* ------------------------------- planning --------------------------------- */

fn valid_user(user: &str) -> bool {
    !user.is_empty()
        && user.len() <= 64
        && user
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '-' | '.'))
}

fn backup_path(paths: &ConsolePaths, index: usize, target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    paths.backups_dir().join(format!("{index:02}-{name}"))
}

fn manager_info(
    manager: SessionManager,
    paths: &ConsolePaths,
    gdm3: bool,
    user: &str,
) -> ManagerInfo {
    let mut info = ManagerInfo {
        id: manager.id().to_string(),
        name: manager.name().to_string(),
        config_paths: Vec::new(),
    };
    info.config_paths = dm_targets(manager, paths, gdm3, user)
        .into_iter()
        .filter(|spec| !user.is_empty() || spec.kind != TargetKind::GdmAccountsSession)
        .map(|spec| spec.path.to_string_lossy().to_string())
        .collect();
    info
}

/// Build the exact enable plan. Pure: reads the filesystem, changes nothing.
pub fn plan_enable(
    manager: SessionManager,
    paths: &ConsolePaths,
    gdm3: bool,
    user: &str,
    staging: &Path,
    fs: &mut dyn PlanFs,
) -> Result<ConsoleSessionPlan, ConsoleSessionError> {
    if !valid_user(user) {
        return Err(ConsoleSessionError::new(
            CODE_INVALID_USER,
            format!(
                "`{user}` is not a valid user name, so Ludex cannot configure autologin for it."
            ),
        ));
    }
    let mut specs = vec![entry_spec(paths)];
    specs.extend(dm_targets(manager, paths, gdm3, user));

    let mut files = Vec::with_capacity(specs.len());
    let mut applied = Vec::with_capacity(specs.len());
    for (index, spec) in specs.into_iter().enumerate() {
        let path = spec.path.to_string_lossy().to_string();
        let existing = fs
            .read(&path)
            .map_err(|error| filesystem_error(&path, error))?;
        let existing_text = existing
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).to_string());
        let contents = render(spec.kind, existing_text.as_deref(), user);
        let backup = existing
            .as_ref()
            .map(|_| backup_path(paths, index, &spec.path))
            .map(|path| path.to_string_lossy().to_string());
        applied.push(AppliedFile {
            path: path.clone(),
            backup: backup.clone(),
            existed: existing.is_some(),
            mode: spec.mode,
        });
        files.push(PlannedFile {
            path,
            action: PlannedAction::Write,
            contents: contents.clone(),
            existed: existing.is_some(),
            backup,
            mode: spec.mode,
            description: spec.description.to_string(),
            bytes: contents.into_bytes(),
            original: existing,
        });
    }

    let state_file = paths.state_file().to_string_lossy().to_string();
    let state_json = serde_json::to_string_pretty(&AppliedState {
        manager,
        gdm3,
        user: user.to_string(),
        enabled: true,
        applied_at: chrono::Utc::now().to_rfc3339(),
        files: applied,
    })
    .map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("Could not serialise the console session state: {error}"),
        )
    })?;
    let commands = commands_for(&files, staging, &state_file, Some(&state_json));

    let mut warnings = Vec::new();
    warnings.push(format!(
        "The administrator password is asked once. Every file is backed up under {} before it is written.",
        paths.backups_dir().display()
    ));
    if manager == SessionManager::Gdm {
        warnings.push(
            "GDM reads the session name from AccountsService, so Ludex updates both files. Reverting restores both byte for byte.".to_string(),
        );
    }
    if manager == SessionManager::Greetd {
        warnings.push(
            "greetd runs `[initial_session]` once at boot, before the greeter. Every other table in config.toml is preserved.".to_string(),
        );
    }

    let message = format!(
        "Ludex will install the console session and configure {} to log {} in automatically. Reboot to test; Disable restores every file.",
        manager.name(),
        user
    );
    Ok(ConsoleSessionPlan {
        enable: true,
        manager: Some(manager_info(manager, paths, gdm3, user)),
        user: Some(user.to_string()),
        session: SESSION_ID.to_string(),
        files,
        state_file,
        commands,
        warnings,
        message,
        state_json: Some(state_json),
    })
}

/// Read the applied state, if any.
pub fn read_state(paths: &ConsolePaths) -> Result<Option<AppliedState>, ConsoleSessionError> {
    let path = paths.state_file();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ConsoleSessionError::new(
                CODE_STATE_INVALID,
                format!("Could not read {}: {error}", path.display()),
            ))
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        ConsoleSessionError::new(
            CODE_STATE_INVALID,
            format!(
                "The console session state at {} is unreadable ({error}). Remove that folder to start over; the files it lists may need restoring by hand.",
                path.display()
            ),
        )
    })
}

/// Every path a revert may touch for a given manager and user. A tampered
/// state file cannot widen this list.
fn revertable_paths(
    mgr: SessionManager,
    paths: &ConsolePaths,
    gdm3: bool,
    user: &str,
) -> BTreeSet<String> {
    let mut allowed: BTreeSet<String> = BTreeSet::new();
    allowed.insert(paths.entry().to_string_lossy().to_string());
    for spec in dm_targets(mgr, paths, gdm3, user) {
        allowed.insert(spec.path.to_string_lossy().to_string());
    }
    // A layout change (`/etc/gdm` vs `/etc/gdm3`) must not make an old revert
    // impossible, and the AccountsService file belongs to the stored user.
    if mgr == SessionManager::Gdm {
        allowed.insert(paths.etc.join(GDM_CONF).to_string_lossy().to_string());
        allowed.insert(paths.etc.join(GDM3_CONF).to_string_lossy().to_string());
        allowed.insert(
            paths
                .var
                .join("lib/AccountsService/users")
                .join(user)
                .to_string_lossy()
                .to_string(),
        );
    }
    allowed
}

/// Build the revert plan from the recorded state. Restores the backups byte for
/// byte and removes the files that did not exist before.
pub fn plan_disable(
    paths: &ConsolePaths,
    staging: &Path,
    fs: &mut dyn PlanFs,
) -> Result<ConsoleSessionPlan, ConsoleSessionError> {
    let state_file = paths.state_file().to_string_lossy().to_string();
    let state_bytes = fs
        .read(&state_file)
        .map_err(|error| filesystem_error(&state_file, error))?
        .ok_or_else(|| {
            ConsoleSessionError::new(
                CODE_NOTHING_TO_REVERT,
                "Ludex has no record of enabling boot-into-console, so there is nothing to restore.",
            )
        })?;
    let state: AppliedState = serde_json::from_slice(&state_bytes).map_err(|error| {
        ConsoleSessionError::new(
            CODE_STATE_INVALID,
            format!(
                "The console session state at {state_file} is unreadable ({error}). Remove that folder to start over; the files it lists may need restoring by hand."
            ),
        )
    })?;
    if !state.enabled {
        return Err(ConsoleSessionError::new(
            CODE_NOTHING_TO_REVERT,
            "Boot-into-console is not marked as enabled, so there is nothing to restore.",
        ));
    }
    let allowed = revertable_paths(state.manager, paths, state.gdm3, &state.user);
    let mut files = Vec::with_capacity(state.files.len());
    for file in &state.files {
        if !allowed.contains(&file.path) {
            return Err(ConsoleSessionError::new(
                CODE_STATE_INVALID,
                format!(
                    "The console session state lists {}, which is not a file Ludex writes. Refusing to restore it.",
                    file.path
                ),
            ));
        }
        if file.existed {
            let backup = file.backup.as_ref().ok_or_else(|| {
                ConsoleSessionError::new(
                    CODE_BACKUP_MISSING,
                    format!(
                        "{} existed before Ludex changed it, but no backup is recorded.",
                        file.path
                    ),
                )
            })?;
            if !Path::new(backup).starts_with(paths.backups_dir()) {
                return Err(ConsoleSessionError::new(
                    CODE_STATE_INVALID,
                    format!(
                        "The console session state points the backup of {} outside {}; refusing to read it.",
                        file.path,
                        paths.backups_dir().display()
                    ),
                ));
            }
            let bytes = fs
                .read(backup)
                .map_err(|error| filesystem_error(backup, error))?
                .ok_or_else(|| {
                    ConsoleSessionError::new(
                        CODE_BACKUP_MISSING,
                        format!(
                            "The backup of {} is missing from {}, so Ludex cannot restore it byte for byte.",
                            file.path,
                            paths.backups_dir().display()
                        ),
                    )
                })?;
            if bytes.len() as u64 > MAX_CONFIG_BYTES {
                return Err(ConsoleSessionError::new(
                    CODE_STATE_INVALID,
                    format!(
                        "The backup of {} is implausibly large; refusing to restore it.",
                        file.path
                    ),
                ));
            }
            files.push(PlannedFile {
                path: file.path.clone(),
                action: PlannedAction::Write,
                contents: String::from_utf8_lossy(&bytes).to_string(),
                existed: true,
                backup: None,
                mode: file.mode,
                description: format!("Restore {}", file.path),
                bytes,
                original: None,
            });
        } else {
            files.push(PlannedFile {
                path: file.path.clone(),
                action: PlannedAction::Remove,
                contents: String::new(),
                existed: false,
                backup: None,
                mode: file.mode,
                description: format!("Remove {} (Ludex created it)", file.path),
                bytes: Vec::new(),
                original: None,
            });
        }
    }
    let commands = commands_for(&files, staging, &state_file, None);
    let restored = files
        .iter()
        .filter(|file| file.action == PlannedAction::Write)
        .count();
    let removed = files.len() - restored;
    let message = format!(
        "Ludex will restore {restored} file(s) and remove {removed} file(s) it created. Reboot to start the previous display manager behaviour again."
    );
    Ok(ConsoleSessionPlan {
        enable: false,
        manager: Some(manager_info(state.manager, paths, state.gdm3, &state.user)),
        user: Some(state.user.clone()),
        session: SESSION_ID.to_string(),
        files,
        state_file,
        commands,
        warnings: vec![
            "The backups are kept under /var/lib/ludex/console-session/backups after a revert, for inspection.".to_string(),
        ],
        message,
        state_json: None,
    })
}

fn install_command(source: &Path, destination: &str, mode: u32) -> String {
    format!(
        "install -D -m {mode:o} {} {}",
        shell_quote(&source.to_string_lossy()),
        shell_quote(destination)
    )
}

fn commands_for(
    files: &[PlannedFile],
    staging: &Path,
    state_file: &str,
    state_json: Option<&str>,
) -> Vec<String> {
    let mut commands = Vec::new();
    // Backups first: a target is never touched before its original is safe.
    for (index, file) in files.iter().enumerate() {
        if let (Some(backup), Some(_)) = (&file.backup, &file.original) {
            commands.push(install_command(
                &staging.join(format!("{index:02}.backup.bin")),
                backup,
                0o644,
            ));
        }
    }
    // The state is written before the targets on enable, so an interrupted
    // first run still knows what to restore.
    if state_json.is_some() {
        commands.push(install_command(
            &staging.join("state.bin"),
            state_file,
            0o644,
        ));
    }
    for (index, file) in files.iter().enumerate() {
        match file.action {
            PlannedAction::Write => commands.push(install_command(
                &staging.join(format!("{index:02}.bin")),
                &file.path,
                file.mode,
            )),
            PlannedAction::Remove => commands.push(format!("rm -f {}", shell_quote(&file.path))),
        }
    }
    if state_json.is_none() {
        commands.push(format!("rm -f {}", shell_quote(state_file)));
    }
    commands
}

/// Apply a plan to a filesystem abstraction. Backups are written before the
/// target; the state file is written last, so a failed run leaves the backups
/// in place for a retry.
pub fn apply(plan: &ConsoleSessionPlan, fs: &mut dyn PlanFs) -> Result<(), ConsoleSessionError> {
    let mut seen = BTreeSet::new();
    for file in &plan.files {
        if !file.path.starts_with('/') {
            return Err(ConsoleSessionError::new(
                CODE_STATE_INVALID,
                format!(
                    "{} is not an absolute path; refusing to write it.",
                    file.path
                ),
            ));
        }
        if !seen.insert(file.path.clone()) {
            return Err(ConsoleSessionError::new(
                CODE_STATE_INVALID,
                format!("The plan writes {} twice.", file.path),
            ));
        }
    }
    // 1. Back every existing original up. A failed backup aborts before any
    //    target changes.
    for (index, file) in plan.files.iter().enumerate() {
        if let (Some(backup), Some(original)) = (&file.backup, &file.original) {
            // 0644: the unprivileged app must read the backup to plan a
            // byte-identical revert. Display manager configuration is not
            // secret.
            fs.write(&format!("{index:02}.backup"), backup, original, 0o644)
                .map_err(|error| filesystem_error(backup, error))?;
        }
    }
    // 2. Record the enabled state before the targets on enable, so a crash
    //    midway still leaves a revert that knows what to restore.
    if let Some(json) = &plan.state_json {
        fs.write("state", &plan.state_file, json.as_bytes(), 0o644)
            .map_err(|error| filesystem_error(&plan.state_file, error))?;
    }
    // 3. Write or remove the targets.
    for (index, file) in plan.files.iter().enumerate() {
        match file.action {
            PlannedAction::Write => fs
                .write(&format!("{index:02}"), &file.path, &file.bytes, file.mode)
                .map_err(|error| filesystem_error(&file.path, error))?,
            PlannedAction::Remove => fs
                .remove(&file.path)
                .map_err(|error| filesystem_error(&file.path, error))?,
        }
    }
    // 4. The revert drops the record last, so a failed restore can be retried.
    if plan.state_json.is_none() {
        fs.remove(&plan.state_file)
            .map_err(|error| filesystem_error(&plan.state_file, error))?;
    }
    Ok(())
}

/* ------------------------------- elevation -------------------------------- */

pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/._-:=@%+".contains(character))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn ensure_pkexec() -> Result<(), ConsoleSessionError> {
    if on_path("pkexec") {
        Ok(())
    } else {
        Err(ConsoleSessionError::new(
            CODE_ELEVATION_UNAVAILABLE,
            "Ludex needs `pkexec` (polkit) to change system files. Install polkit and try again.",
        ))
    }
}

fn run_pkexec(args: &[&str]) -> Result<(), ConsoleSessionError> {
    ensure_pkexec()?;
    let status = Command::new("pkexec")
        .args(args)
        .status()
        .map_err(|error| {
            ConsoleSessionError::new(
                CODE_ELEVATION_FAILED,
                format!("Could not start `pkexec`: {error}"),
            )
        })?;
    match status.code() {
        Some(0) => Ok(()),
        Some(126) => Err(ConsoleSessionError::new(
            CODE_ELEVATION_CANCELLED,
            "The administrator prompt was cancelled; nothing was changed.",
        )),
        Some(127) => Err(ConsoleSessionError::new(
            CODE_ELEVATION_FAILED,
            "Polkit refused the request. Your account is not allowed to change display manager configuration.",
        )),
        Some(code) => Err(ConsoleSessionError::new(
            CODE_ELEVATION_FAILED,
            format!("The elevated command failed with exit code {code}; check the Ludex log for the command it ran."),
        )),
        None => Err(ConsoleSessionError::new(
            CODE_ELEVATION_FAILED,
            "The elevated command was terminated by a signal.",
        )),
    }
}

/// Stage every write in a private directory and run one `pkexec` shell with the
/// exact commands the plan lists.
struct ElevatedFs {
    staging: PathBuf,
    ops: Vec<String>,
}

impl ElevatedFs {
    fn new(staging: &Path) -> Self {
        Self {
            staging: staging.to_path_buf(),
            ops: Vec::new(),
        }
    }

    fn script(&self) -> String {
        let mut script = String::from("set -eu\n");
        for op in &self.ops {
            script.push_str(op);
            script.push('\n');
        }
        script
    }
}

impl PlanFs for ElevatedFs {
    fn read(&mut self, _path: &str) -> io::Result<Option<Vec<u8>>> {
        Err(io::Error::other(
            "the elevated filesystem does not read; the plan was built before staging",
        ))
    }

    fn write(&mut self, slot: &str, path: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
        let staged = self.staging.join(format!("{slot}.bin"));
        std::fs::write(&staged, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
        }
        self.ops.push(install_command(&staged, path, mode));
        Ok(())
    }

    fn remove(&mut self, path: &str) -> io::Result<()> {
        self.ops.push(format!("rm -f {}", shell_quote(path)));
        Ok(())
    }
}

/// Run a plan through polkit. The unprivileged process only writes staging
/// files; the elevated shell runs `install`/`rm` with quoted absolute paths.
pub fn apply_elevated(plan: &ConsoleSessionPlan) -> Result<(), ConsoleSessionError> {
    ensure_pkexec()?;
    let staging = staging_dir();
    if staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&staging).map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!(
                "Could not create the staging folder {}: {error}",
                staging.display()
            ),
        )
    })?;

    let mut fs = ElevatedFs::new(&staging);
    let staged = apply(plan, &mut fs);
    let script = fs.script();
    let result = match staged {
        Ok(()) => run_pkexec(&["/bin/sh", "-c", &script]),
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/* -------------------------------- entry ----------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPlan {
    /// `missing`, `installed` or `differs`.
    pub state: String,
    pub destination: String,
    pub contents: String,
    pub install_command: String,
    pub uninstall_command: String,
    pub message: String,
}

pub fn entry_state_for(existing: Option<&[u8]>) -> &'static str {
    match existing {
        None => "missing",
        Some(bytes) if bytes == SESSION_ENTRY.as_bytes() => "installed",
        Some(_) => "differs",
    }
}

pub fn entry_plan(paths: &ConsolePaths) -> EntryPlan {
    let destination = paths.entry().to_string_lossy().to_string();
    let existing = std::fs::read(paths.entry()).ok();
    let state = entry_state_for(existing.as_deref()).to_string();
    let staged = staging_dir().join(SESSION_ENTRY_NAME);
    let message = match state.as_str() {
        "missing" => "The console session is not installed. Install it so it appears in the display manager's session list.".to_string(),
        "installed" => "The console session is installed and matches this Ludex build.".to_string(),
        _ => format!(
            "{destination} already exists with different contents. Ludex will not overwrite another program's session file."
        ),
    };
    EntryPlan {
        state,
        destination: destination.clone(),
        contents: SESSION_ENTRY.to_string(),
        install_command: format!(
            "pkexec install -D -m 644 {} {}",
            shell_quote(&staged.to_string_lossy()),
            shell_quote(&destination)
        ),
        uninstall_command: format!("pkexec rm -f {}", shell_quote(&destination)),
        message,
    }
}

fn stage_entry() -> Result<PathBuf, ConsoleSessionError> {
    let staging = staging_dir();
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&staging).map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!(
                "Could not create the staging folder {}: {error}",
                staging.display()
            ),
        )
    })?;
    let file = staging.join(SESSION_ENTRY_NAME);
    std::fs::write(&file, SESSION_ENTRY.as_bytes()).map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("Could not stage the session entry: {error}"),
        )
    })?;
    Ok(file)
}

pub fn install_entry(paths: &ConsolePaths) -> Result<EntryPlan, ConsoleSessionError> {
    ensure_pkexec()?;
    let plan = entry_plan(paths);
    match plan.state.as_str() {
        "installed" => return Ok(plan),
        "differs" => return Err(ConsoleSessionError::new(CODE_ENTRY_CONFLICT, plan.message)),
        _ => {}
    }
    let staged = stage_entry()?;
    let staged = staged.to_string_lossy().to_string();
    let destination = paths.entry().to_string_lossy().to_string();
    let result = run_pkexec(&["install", "-D", "-m", "644", &staged, &destination]);
    let _ = std::fs::remove_dir_all(staging_dir());
    result?;
    Ok(entry_plan(paths))
}

pub fn uninstall_entry(paths: &ConsolePaths) -> Result<EntryPlan, ConsoleSessionError> {
    let plan = entry_plan(paths);
    match plan.state.as_str() {
        "missing" => return Ok(plan),
        "differs" => {
            return Err(ConsoleSessionError::new(
                CODE_ENTRY_CONFLICT,
                format!(
                    "{} is not the session file Ludex ships, so Ludex will not delete it.",
                    plan.destination
                ),
            ))
        }
        _ => {}
    }
    run_pkexec(&["rm", "-f", &plan.destination])?;
    Ok(entry_plan(paths))
}

/* --------------------------------- status --------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SeatSession {
    pub session_type: Option<String>,
    pub active: Option<bool>,
    /// `Type=wayland` and `Active=yes`, which the Sunshine capture path needs.
    pub compatible: bool,
    pub note: String,
}

pub fn parse_seat_session(text: &str) -> (Option<String>, Option<bool>) {
    let mut session_type = None;
    let mut active = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Type=") {
            session_type = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Active=") {
            active = Some(value.trim().eq_ignore_ascii_case("yes"));
        }
    }
    (session_type, active)
}

pub fn seat_from(session_type: Option<String>, active: Option<bool>) -> SeatSession {
    let compatible = session_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        && active == Some(true);
    let note = if compatible {
        "The current session reports Type=wayland and Active=yes, the state Sunshine's KMS capture needs.".to_string()
    } else if let (Some(kind), Some(active)) = (&session_type, active) {
        format!(
            "The current session reports Type={kind} and Active={}. Sunshine's KMS capture needs loginctl to report Type=wayland and Active=yes, otherwise the stream can be black. Check with `loginctl show-session $XDG_SESSION_ID -p Type -p Active`.",
            if active { "yes" } else { "no" }
        )
    } else {
        "Ludex could not read the seat session (loginctl). Sunshine's KMS capture needs Type=wayland and Active=yes; check with `loginctl show-session $XDG_SESSION_ID -p Type -p Active`.".to_string()
    };
    SeatSession {
        session_type,
        active,
        compatible,
        note,
    }
}

pub fn probe_seat() -> SeatSession {
    if crate::window_policy::current_platform() != Platform::Linux {
        return SeatSession {
            note: "The seat session is only reported on Linux.".to_string(),
            ..SeatSession::default()
        };
    }
    let Ok(id) = std::env::var("XDG_SESSION_ID") else {
        return seat_from(None, None);
    };
    let output = Command::new("loginctl")
        .args(["show-session", &id, "-p", "Type", "-p", "Active"])
        .output();
    let Ok(output) = output else {
        return seat_from(None, None);
    };
    if !output.status.success() {
        return seat_from(None, None);
    }
    let (session_type, active) = parse_seat_session(&String::from_utf8_lossy(&output.stdout));
    seat_from(session_type, active)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleSessionStatus {
    /// `linux` or the host platform id.
    pub platform: String,
    pub supported: bool,
    pub manager: Option<ManagerInfo>,
    /// The refusal sentence when no manager was detected.
    pub manager_reason: Option<String>,
    pub user: Option<String>,
    pub session: String,
    pub entry_path: String,
    pub entry_state: String,
    pub entry_installed: bool,
    pub enabled: bool,
    pub managed_files: Vec<String>,
    pub seat: SeatSession,
    pub pkexec_available: bool,
    pub message: String,
}

fn current_user_from_passwd(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 3 && fields[2].trim().parse::<u32>().ok() == Some(uid) {
            return Some(fields[0].to_string());
        }
    }
    None
}

fn current_uid_from_status(status: &str) -> Option<u32> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// The effective user name, from `/proc/self/status` + `/etc/passwd` so it is
/// the account the session runs as, not whatever `$USER` a launcher exported.
pub fn current_user() -> Result<String, ConsoleSessionError> {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let uid = current_uid_from_status(&status);
    if let Some(user) = uid.and_then(|uid| current_user_from_passwd(&passwd, uid)) {
        if valid_user(&user) {
            return Ok(user);
        }
    }
    for key in ["USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            if valid_user(&value) {
                return Ok(value);
            }
        }
    }
    Err(ConsoleSessionError::new(
        CODE_INVALID_USER,
        "Ludex could not determine the user this session runs as, so it cannot configure autologin.",
    ))
}

pub fn status(
    paths: &ConsolePaths,
    detection: Detection,
    seat: SeatSession,
) -> Result<ConsoleSessionStatus, ConsoleSessionError> {
    let supported = crate::window_policy::current_platform() == Platform::Linux;
    let entry_existing = std::fs::read(paths.entry()).ok();
    let entry_state = entry_state_for(entry_existing.as_deref()).to_string();
    let state = read_state(paths)?;
    let enabled = state.as_ref().is_some_and(|value| value.enabled);
    let managed_files = state
        .as_ref()
        .map(|value| value.files.iter().map(|file| file.path.clone()).collect())
        .unwrap_or_default();
    let user = state
        .as_ref()
        .map(|value| value.user.clone())
        .or_else(|| current_user().ok());
    // An enabled state remembers the manager even if detection now fails
    // (the package was removed, the symlink is missing). The switch must still
    // be able to disable what it enabled.
    let manager_id = detection
        .manager
        .or_else(|| state.as_ref().map(|value| value.manager));
    let gdm3 = state
        .as_ref()
        .map(|value| value.gdm3)
        .unwrap_or(detection.gdm3);
    let manager =
        manager_id.map(|manager| manager_info(manager, paths, gdm3, user.as_deref().unwrap_or("")));
    let message = if !supported {
        "Boot-into-console is only implemented for Linux display managers.".to_string()
    } else if enabled {
        format!(
            "Boot-into-console is enabled{}. Reboot to start the console session; Disable restores the previous files.",
            manager_id
                .map(|manager| format!(" through {}", manager.name()))
                .unwrap_or_default()
        )
    } else if let Some(manager) = manager_id {
        format!(
            "{} detected. Ludex can make the console session the boot target; every file it changes is backed up first.",
            manager.name()
        )
    } else {
        detection.reason.clone()
    };
    Ok(ConsoleSessionStatus {
        platform: if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else if supported {
            "linux"
        } else {
            "other"
        }
        .to_string(),
        supported,
        manager,
        manager_reason: if detection.manager.is_none() && !enabled {
            Some(detection.reason.clone())
        } else {
            None
        },
        user,
        session: SESSION_ID.to_string(),
        entry_path: paths.entry().to_string_lossy().to_string(),
        entry_installed: entry_state == "installed",
        entry_state,
        enabled,
        managed_files,
        seat,
        pkexec_available: on_path("pkexec"),
        message,
    })
}

/* -------------------------------- commands -------------------------------- */

fn require_linux() -> Result<(), ConsoleSessionError> {
    if crate::window_policy::current_platform() == Platform::Linux {
        Ok(())
    } else {
        Err(ConsoleSessionError::new(
            CODE_UNSUPPORTED,
            "Booting straight into console mode is only implemented for Linux display managers.",
        ))
    }
}

/// The detection, the shipped entry state and the current seat, for Settings.
#[tauri::command]
pub async fn console_session_status() -> Result<ConsoleSessionStatus, ConsoleSessionError> {
    tokio::task::spawn_blocking(|| {
        let paths = ConsolePaths::system();
        let detection = probe_managers(&paths);
        let seat = probe_seat();
        status(&paths, detection, seat)
    })
    .await
    .map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("The console session probe failed: {error}"),
        )
    })?
}

/// Dry-run: the exact files, contents and commands, without touching anything.
#[tauri::command]
pub async fn console_session_plan(enable: bool) -> Result<ConsoleSessionPlan, ConsoleSessionError> {
    let paths = ConsolePaths::system();
    let staging = staging_dir();
    tokio::task::spawn_blocking(move || {
        if !enable {
            return plan_disable(&paths, &staging, &mut ReadOnlyFs);
        }
        require_linux()?;
        let detection = probe_managers(&paths);
        let manager = detection
            .manager
            .ok_or_else(|| ConsoleSessionError::new(CODE_NO_MANAGER, detection.reason.clone()))?;
        if read_state(&paths)?.is_some_and(|state| state.enabled) {
            return Err(ConsoleSessionError::new(
                CODE_ALREADY_ENABLED,
                "Boot-into-console is already enabled. Disable it before enabling it again.",
            ));
        }
        let user = current_user()?;
        plan_enable(
            manager,
            &paths,
            detection.gdm3,
            &user,
            &staging,
            &mut ReadOnlyFs,
        )
    })
    .await
    .map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("The console session plan failed: {error}"),
        )
    })?
}

/// Apply the freshly computed plan through one polkit prompt. `confirm` is the
/// consent the dialog collected; the backend never trusts a client plan.
#[tauri::command]
pub async fn console_session_apply(
    enable: bool,
    confirm: bool,
) -> Result<ConsoleSessionStatus, ConsoleSessionError> {
    if !confirm {
        return Err(ConsoleSessionError::new(
            CODE_CONSENT,
            "Changing display manager configuration needs explicit consent: review the plan first.",
        ));
    }
    let paths = ConsolePaths::system();
    let staging = staging_dir();
    tokio::task::spawn_blocking(move || {
        let plan = if enable {
            require_linux()?;
            let detection = probe_managers(&paths);
            let manager = detection.manager.ok_or_else(|| {
                ConsoleSessionError::new(CODE_NO_MANAGER, detection.reason.clone())
            })?;
            if read_state(&paths)?.is_some_and(|state| state.enabled) {
                return Err(ConsoleSessionError::new(
                    CODE_ALREADY_ENABLED,
                    "Boot-into-console is already enabled. Disable it before enabling it again.",
                ));
            }
            let user = current_user()?;
            plan_enable(
                manager,
                &paths,
                detection.gdm3,
                &user,
                &staging,
                &mut ReadOnlyFs,
            )?
        } else {
            plan_disable(&paths, &staging, &mut ReadOnlyFs)?
        };
        apply_elevated(&plan)?;
        let detection = probe_managers(&paths);
        let seat = probe_seat();
        status(&paths, detection, seat)
    })
    .await
    .map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("The console session change failed: {error}"),
        )
    })?
}

#[tauri::command]
pub async fn console_session_entry_plan() -> Result<EntryPlan, ConsoleSessionError> {
    tokio::task::spawn_blocking(|| entry_plan(&ConsolePaths::system()))
        .await
        .map_err(|error| {
            ConsoleSessionError::new(
                CODE_FAILED,
                format!("The session entry probe failed: {error}"),
            )
        })
}

#[tauri::command]
pub async fn console_session_install_entry(
    confirm: bool,
) -> Result<EntryPlan, ConsoleSessionError> {
    if !confirm {
        return Err(ConsoleSessionError::new(
            CODE_CONSENT,
            "Installing the session entry needs explicit consent.",
        ));
    }
    tokio::task::spawn_blocking(|| {
        require_linux()?;
        install_entry(&ConsolePaths::system())
    })
    .await
    .map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("The session entry install failed: {error}"),
        )
    })?
}

#[tauri::command]
pub async fn console_session_uninstall_entry(
    confirm: bool,
) -> Result<EntryPlan, ConsoleSessionError> {
    if !confirm {
        return Err(ConsoleSessionError::new(
            CODE_CONSENT,
            "Removing the session entry needs explicit consent.",
        ));
    }
    tokio::task::spawn_blocking(|| {
        require_linux()?;
        uninstall_entry(&ConsolePaths::system())
    })
    .await
    .map_err(|error| {
        ConsoleSessionError::new(
            CODE_FAILED,
            format!("The session entry removal failed: {error}"),
        )
    })?
}

/* ---------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryFs {
        files: std::collections::BTreeMap<String, (Vec<u8>, u32)>,
    }

    impl MemoryFs {
        fn with(mut self, path: &str, contents: &str) -> Self {
            self.files
                .insert(path.to_string(), (contents.as_bytes().to_vec(), 0o644));
            self
        }

        fn text(&self, path: &str) -> Option<String> {
            self.files
                .get(path)
                .map(|(bytes, _)| String::from_utf8_lossy(bytes).to_string())
        }
    }

    impl PlanFs for MemoryFs {
        fn read(&mut self, path: &str) -> io::Result<Option<Vec<u8>>> {
            Ok(self.files.get(path).map(|(bytes, _)| bytes.clone()))
        }

        fn write(&mut self, _slot: &str, path: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
            self.files.insert(path.to_string(), (bytes.to_vec(), mode));
            Ok(())
        }

        fn remove(&mut self, path: &str) -> io::Result<()> {
            self.files.remove(path);
            Ok(())
        }
    }

    fn paths() -> ConsolePaths {
        ConsolePaths::under(Path::new("/scratch"))
    }

    fn staging() -> PathBuf {
        PathBuf::from("/tmp/ludex-console-session-test")
    }

    fn entry_path() -> String {
        paths().entry().to_string_lossy().to_string()
    }

    fn plan_for(
        manager: SessionManager,
        gdm3: bool,
        user: &str,
        fs: &mut MemoryFs,
    ) -> ConsoleSessionPlan {
        plan_enable(manager, &paths(), gdm3, user, &staging(), fs).unwrap()
    }

    #[test]
    fn the_shipped_entry_follows_the_wayland_session_spec() {
        for line in [
            "[Desktop Entry]",
            "Type=Application",
            "Name=Ludex Console",
            "Exec=ludex --console",
        ] {
            assert!(SESSION_ENTRY.lines().any(|value| value == line), "{line}");
        }
        assert!(SESSION_ENTRY.contains("Comment="));
        assert!(
            !SESSION_ENTRY.contains("X-GNOME-Autostart"),
            "not an autostart file"
        );
        assert!(SESSION_ENTRY.ends_with('\n'));
    }

    #[test]
    fn detection_prefers_the_systemd_target_and_refuses_ambiguity() {
        let sddm = detect_from(
            &ManagerProbe {
                display_manager_target: Some("/usr/lib/systemd/system/sddm.service".to_string()),
                default_display_manager: None,
                binaries: vec!["gdm".to_string()],
            },
            false,
        );
        assert_eq!(sddm.manager, Some(SessionManager::Sddm));
        assert!(sddm.reason.is_empty());
        assert!(sddm.evidence[0].contains("sddm.service"));

        let debian = detect_from(
            &ManagerProbe {
                display_manager_target: None,
                default_display_manager: Some("/usr/sbin/gdm3".to_string()),
                binaries: Vec::new(),
            },
            true,
        );
        assert_eq!(debian.manager, Some(SessionManager::Gdm));
        assert!(debian.gdm3);

        let greetd = detect_from(
            &ManagerProbe {
                binaries: vec!["greetd".to_string(), "regreet".to_string()],
                ..ManagerProbe::default()
            },
            false,
        );
        assert_eq!(greetd.manager, Some(SessionManager::Greetd));

        let ambiguous = detect_from(
            &ManagerProbe {
                binaries: vec!["sddm".to_string(), "lightdm".to_string()],
                ..ManagerProbe::default()
            },
            false,
        );
        assert_eq!(ambiguous.manager, None);
        assert!(ambiguous.reason.contains("Several display managers"));
        assert!(ambiguous.reason.contains("SDDM"));
        assert!(ambiguous.reason.contains("LightDM"));

        let none = detect_from(&ManagerProbe::default(), false);
        assert_eq!(none.manager, None);
        assert!(none.reason.contains("no display manager"));
        assert!(none.reason.contains("cannot make the console session"));
    }

    #[test]
    fn sddm_merges_autologin_and_keeps_foreign_keys() {
        let conf = "/scratch/etc/sddm.conf.d/90-ludex-console.conf";
        let mut fs = MemoryFs::default().with(
            conf,
            "[Theme]\nCurrent=breeze\n\n[Autologin]\nUser=someone\nSession=plasma\n",
        );
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut fs);
        let file = plan.files.iter().find(|file| file.path == conf).unwrap();
        assert!(file.contents.contains("[Theme]\nCurrent=breeze"));
        assert!(file.contents.contains("User=player"));
        assert!(file.contents.contains("Session=ludex-console"));
        assert!(file.contents.contains("Relogin=true"));
        assert!(!file.contents.contains("Session=plasma"));
        assert!(file.existed);
        assert!(file.backup.is_some());
        assert!(file.contents.contains("User=player\n"));
        assert_eq!(file.contents.matches("User=").count(), 1);
    }

    #[test]
    fn duplicate_keys_do_not_override_the_new_value() {
        let merged = merge_ini(
            Some("[Autologin]\nUser=one\nUser=two\n"),
            "Autologin",
            &[("User", "player")],
        );
        assert_eq!(merged.matches("User=player").count(), 1);
        assert!(!merged.contains("User=one"));
        assert!(!merged.contains("User=two"));
    }

    #[test]
    fn sddm_without_an_existing_file_writes_the_section() {
        let mut fs = MemoryFs::default();
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut fs);
        let file = plan
            .files
            .iter()
            .find(|file| file.path.ends_with(SDDM_CONF))
            .unwrap();
        assert!(!file.existed);
        assert!(file.backup.is_none());
        assert!(file.contents.starts_with("[Autologin]\n"));
    }

    #[test]
    fn gdm_writes_custom_conf_and_accounts_service() {
        for (gdm3, suffix) in [(false, GDM_CONF), (true, GDM3_CONF)] {
            let mut fs = MemoryFs::default().with(
                "/scratch/var/lib/AccountsService/users/player",
                "[User]\nLanguage=en_GB\nSession=gnome\n",
            );
            let plan = plan_for(SessionManager::Gdm, gdm3, "player", &mut fs);
            let custom = plan
                .files
                .iter()
                .find(|file| file.path.ends_with(suffix))
                .unwrap();
            assert!(custom.contents.contains("AutomaticLoginEnable=true"));
            assert!(custom.contents.contains("AutomaticLogin=player"));
            assert!(custom.contents.contains("WaylandEnable=true"));
            let accounts = plan
                .files
                .iter()
                .find(|file| file.path.ends_with("AccountsService/users/player"))
                .unwrap();
            assert!(accounts.contents.contains("Language=en_GB"));
            assert!(accounts.contents.contains("Session=ludex-console"));
            assert!(!accounts.contents.contains("Session=gnome"));
            assert_eq!(accounts.contents.matches("Session=").count(), 1);
        }
    }

    #[test]
    fn greetd_replaces_initial_session_and_preserves_the_greeter() {
        let conf = "/scratch/etc/greetd/config.toml";
        let mut fs = MemoryFs::default().with(
            conf,
            "[terminal]\nvt = 1\n\n[default_session]\ncommand = \"regreet\"\nuser = \"greeter\"\n",
        );
        let plan = plan_for(SessionManager::Greetd, false, "player", &mut fs);
        let file = plan.files.iter().find(|file| file.path == conf).unwrap();
        assert!(file.contents.contains("[terminal]\nvt = 1"));
        assert!(file
            .contents
            .contains("[default_session]\ncommand = \"regreet\""));
        assert!(file
            .contents
            .contains("[initial_session]\ncommand = \"ludex --console\"\nuser = \"player\""));
    }

    #[test]
    fn greetd_existing_initial_session_is_replaced_once() {
        let conf = "/scratch/etc/greetd/config.toml";
        let mut fs = MemoryFs::default().with(
            conf,
            "[initial_session]\ncommand = \"old-thing\"\nuser = \"old\"\n\n[terminal]\nvt = 2\n",
        );
        let plan = plan_for(SessionManager::Greetd, false, "player", &mut fs);
        let file = plan.files.iter().find(|file| file.path == conf).unwrap();
        assert!(!file.contents.contains("old-thing"));
        assert_eq!(file.contents.matches("[initial_session]").count(), 1);
        assert!(file.contents.contains("[terminal]\nvt = 2"));
    }

    #[test]
    fn lightdm_writes_the_seat_drop_in() {
        let mut fs = MemoryFs::default();
        let plan = plan_for(SessionManager::Lightdm, false, "player", &mut fs);
        let file = plan
            .files
            .iter()
            .find(|file| file.path.ends_with(LIGHTDM_CONF))
            .unwrap();
        assert!(file.contents.starts_with("[Seat:*]\n"));
        assert!(file.contents.contains("autologin-user=player"));
        assert!(file.contents.contains("autologin-user-timeout=0"));
        assert!(file.contents.contains("autologin-session=ludex-console"));
        assert!(file.contents.contains("user-session=ludex-console"));
    }

    #[test]
    fn revert_restores_every_byte_and_removes_created_files() {
        let mut fs = MemoryFs::default().with(
            "/scratch/etc/sddm.conf.d/90-ludex-console.conf",
            "[Autologin]\nUser=someone\nSession=plasma",
        );
        let original = fs
            .text("/scratch/etc/sddm.conf.d/90-ludex-console.conf")
            .unwrap();
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut fs);
        apply(&plan, &mut fs).unwrap();

        assert!(fs
            .text("/scratch/etc/sddm.conf.d/90-ludex-console.conf")
            .unwrap()
            .contains("User=player"));
        let state = fs.text(&plan.state_file).unwrap();
        assert!(state.contains("sddm"));
        assert!(fs.files.contains_key(&entry_path()), "entry installed");

        let revert = plan_disable(&paths(), &staging(), &mut fs).unwrap();
        assert!(!revert.enable);
        apply(&revert, &mut fs).unwrap();

        assert_eq!(
            fs.text("/scratch/etc/sddm.conf.d/90-ludex-console.conf")
                .unwrap(),
            original,
            "byte-identical restore"
        );
        assert!(
            !fs.files.contains_key(&entry_path()),
            "created entry removed"
        );
        assert!(!fs.files.contains_key(&plan.state_file), "state removed");
    }

    #[test]
    fn revert_deletes_a_config_that_did_not_exist_before() {
        let mut fs = MemoryFs::default();
        let plan = plan_for(SessionManager::Lightdm, false, "player", &mut fs);
        apply(&plan, &mut fs).unwrap();
        let conf = "/scratch/etc/lightdm/lightdm.conf.d/50-ludex-console.conf";
        assert!(fs.files.contains_key(conf));
        let revert = plan_disable(&paths(), &staging(), &mut fs).unwrap();
        apply(&revert, &mut fs).unwrap();
        assert!(!fs.files.contains_key(conf));
    }

    #[test]
    fn backups_and_state_are_written_before_any_target() {
        struct Order(Vec<String>);
        impl PlanFs for Order {
            fn read(&mut self, _path: &str) -> io::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn write(
                &mut self,
                slot: &str,
                path: &str,
                _bytes: &[u8],
                _mode: u32,
            ) -> io::Result<()> {
                self.0.push(format!("{slot}:{path}"));
                Ok(())
            }
            fn remove(&mut self, path: &str) -> io::Result<()> {
                self.0.push(format!("rm:{path}"));
                Ok(())
            }
        }
        let conf = "/scratch/etc/sddm.conf.d/90-ludex-console.conf";
        let mut memory = MemoryFs::default().with(conf, "[Autologin]\nUser=old\n");
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut memory);
        let mut order = Order(Vec::new());
        apply(&plan, &mut order).unwrap();
        let backup = order
            .0
            .iter()
            .position(|op| op.starts_with("01.backup:"))
            .unwrap();
        let state = order
            .0
            .iter()
            .position(|op| op.starts_with("state:"))
            .unwrap();
        let target = order
            .0
            .iter()
            .position(|op| op == &format!("01:{conf}"))
            .unwrap();
        assert!(backup < state, "backup before the state record");
        assert!(state < target, "state record before the target write");
        assert!(order.0.last().unwrap().starts_with("01:"));
    }

    #[test]
    fn the_dry_run_changes_nothing_and_lists_the_exact_commands() {
        let mut fs = MemoryFs::default().with(
            "/scratch/etc/lightdm/lightdm.conf.d/50-ludex-console.conf",
            "[Seat:*]\n",
        );
        let before: Vec<String> = fs.files.keys().cloned().collect();
        let plan = plan_for(SessionManager::Lightdm, false, "player", &mut fs);
        let after: Vec<String> = fs.files.keys().cloned().collect();
        assert_eq!(before, after, "the plan is read-only");
        assert!(plan
            .commands
            .iter()
            .any(|command| command.starts_with("install -D -m 644 ")));
        assert!(plan
            .commands
            .iter()
            .any(|command| command.contains("50-ludex-console.conf")));
        assert!(plan
            .commands
            .iter()
            .any(|command| command.starts_with("install -D -m 644 ")));
        assert!(
            plan.files
                .iter()
                .filter(|file| file.existed)
                .all(|file| file.backup.is_some()),
            "existing files are backed up"
        );

        // The commands are the operations apply performs, in the same order.
        struct Recorder(Vec<String>);
        impl PlanFs for Recorder {
            fn read(&mut self, _path: &str) -> io::Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn write(
                &mut self,
                slot: &str,
                path: &str,
                _bytes: &[u8],
                mode: u32,
            ) -> io::Result<()> {
                self.0
                    .push(format!("install -D -m {mode:o} <{slot}> {path}"));
                Ok(())
            }
            fn remove(&mut self, path: &str) -> io::Result<()> {
                self.0.push(format!("rm -f {path}"));
                Ok(())
            }
        }
        let mut recorder = Recorder(Vec::new());
        apply(&plan, &mut recorder).unwrap();
        assert_eq!(recorder.0.len(), plan.commands.len());
        for (command, operation) in plan.commands.iter().zip(recorder.0.iter()) {
            // `<slot>` in the recorder stands for the staged file path; the
            // tail (target and mode) must match exactly.
            assert_eq!(
                command.split_whitespace().take(4).collect::<Vec<_>>(),
                operation.split_whitespace().take(4).collect::<Vec<_>>()
            );
            assert!(command.ends_with(operation.split_whitespace().last().unwrap()));
        }
    }

    #[test]
    fn disable_without_state_refuses_with_a_sentence() {
        let mut fs = MemoryFs::default();
        let error = plan_disable(&paths(), &staging(), &mut fs).unwrap_err();
        assert_eq!(error.code, CODE_NOTHING_TO_REVERT);
        assert!(error.message.contains("no record"));
    }

    #[test]
    fn a_tampered_state_file_cannot_restore_an_arbitrary_path() {
        let mut fs = MemoryFs::default();
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut fs);
        apply(&plan, &mut fs).unwrap();
        let mut state: AppliedState =
            serde_json::from_str(&fs.text(&plan.state_file).unwrap()).unwrap();
        state.files.push(AppliedFile {
            path: "/etc/passwd".to_string(),
            backup: Some("/scratch/var/lib/ludex/console-session/backups/99-passwd".to_string()),
            existed: true,
            mode: 0o644,
        });
        fs.write(
            "state",
            &plan.state_file,
            serde_json::to_string(&state).unwrap().as_bytes(),
            0o644,
        )
        .unwrap();
        let error = plan_disable(&paths(), &staging(), &mut fs).unwrap_err();
        assert_eq!(error.code, CODE_STATE_INVALID);
        assert!(error.message.contains("/etc/passwd"));
        assert!(error.message.contains("Refusing"));
    }

    #[test]
    fn a_missing_backup_refuses_instead_of_deleting() {
        let mut fs = MemoryFs::default().with(
            "/scratch/etc/sddm.conf.d/90-ludex-console.conf",
            "[Autologin]\nUser=someone\n",
        );
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut fs);
        apply(&plan, &mut fs).unwrap();
        let backup = plan
            .files
            .iter()
            .find_map(|file| file.backup.clone())
            .unwrap();
        fs.files.remove(&backup);
        let error = plan_disable(&paths(), &staging(), &mut fs).unwrap_err();
        assert_eq!(error.code, CODE_BACKUP_MISSING);
        assert!(error.message.contains("cannot restore it byte for byte"));
    }

    #[test]
    fn the_state_round_trips_with_camel_case_fields() {
        let mut fs = MemoryFs::default();
        let plan = plan_for(SessionManager::Sddm, false, "player", &mut fs);
        apply(&plan, &mut fs).unwrap();
        let text = fs.text(&plan.state_file).unwrap();
        assert!(text.contains("\"enabled\": true"));
        assert!(text.contains("\"appliedAt\""));
        let state: AppliedState = serde_json::from_str(&text).unwrap();
        assert_eq!(state.manager, SessionManager::Sddm);
        assert_eq!(state.user, "player");
        assert!(state.enabled);
    }

    #[test]
    fn entry_state_and_install_command_are_exact() {
        assert_eq!(entry_state_for(None), "missing");
        assert_eq!(entry_state_for(Some(SESSION_ENTRY.as_bytes())), "installed");
        assert_eq!(entry_state_for(Some(b"other")), "differs");

        let paths = paths();
        let plan = entry_plan(&paths);
        assert!(plan
            .install_command
            .starts_with("pkexec install -D -m 644 "));
        assert!(plan
            .install_command
            .ends_with("/scratch/usr/share/wayland-sessions/ludex-console.desktop"));
        assert_eq!(
            plan.uninstall_command,
            "pkexec rm -f /scratch/usr/share/wayland-sessions/ludex-console.desktop"
        );
        assert_eq!(plan.contents, SESSION_ENTRY);
    }

    #[test]
    fn shell_quoting_never_lets_a_path_escape() {
        assert_eq!(
            shell_quote("/etc/sddm.conf.d/90-ludex-console.conf"),
            "/etc/sddm.conf.d/90-ludex-console.conf"
        );
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("/tmp/it's"), "'/tmp/it'\\''s'");
    }

    #[test]
    fn user_names_are_validated_before_they_become_paths() {
        assert!(valid_user("player"));
        assert!(valid_user("player-1"));
        assert!(!valid_user(""));
        assert!(!valid_user("../root"));
        assert!(!valid_user("a/b"));
        let error = plan_enable(
            SessionManager::Gdm,
            &paths(),
            false,
            "../root",
            &staging(),
            &mut MemoryFs::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, CODE_INVALID_USER);
    }

    #[test]
    fn the_current_user_is_read_from_passwd_not_the_environment() {
        let passwd =
            "root:x:0:0:root:/root:/bin/bash\nplayer:x:1000:1000::/home/player:/bin/bash\n";
        assert_eq!(
            current_user_from_passwd(passwd, 1000).as_deref(),
            Some("player")
        );
        assert_eq!(current_user_from_passwd(passwd, 42), None);
        let status = "Name:\tludex\nUid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(current_uid_from_status(status), Some(1000));
        assert_eq!(current_uid_from_status(""), None);
    }

    #[test]
    fn the_seat_matrix_matches_the_loginctl_requirement() {
        let (kind, active) = parse_seat_session("Type=wayland\nActive=yes\n");
        assert_eq!(kind.as_deref(), Some("wayland"));
        assert_eq!(active, Some(true));
        assert!(seat_from(kind, active).compatible);

        let x11 = seat_from(Some("x11".to_string()), Some(true));
        assert!(!x11.compatible);
        assert!(x11.note.contains("Type=x11"));
        assert!(x11.note.contains("black"));

        let inactive = seat_from(Some("wayland".to_string()), Some(false));
        assert!(!inactive.compatible);

        let unknown = seat_from(None, None);
        assert!(!unknown.compatible);
        assert!(unknown.note.contains("loginctl show-session"));
    }

    #[test]
    fn a_plan_never_lists_the_same_path_twice() {
        let mut fs = MemoryFs::default();
        let plan = plan_for(SessionManager::Gdm, false, "player", &mut fs);
        let unique: BTreeSet<&String> = plan.files.iter().map(|file| &file.path).collect();
        assert_eq!(unique.len(), plan.files.len());
        assert!(plan.manager.as_ref().unwrap().config_paths.len() >= 2);
        assert_eq!(plan.session, SESSION_ID);
    }
}
