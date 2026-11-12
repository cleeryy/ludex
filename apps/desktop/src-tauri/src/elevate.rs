//! Shared one-shot elevation for administrator-only actions (#192).
//!
//! Ludex never runs itself as administrator and never keeps a privileged
//! helper alive. A privileged action is described by an [`ElevationRequest`]
//! whose [`ElevationOperation`] is a closed enum: the webview cannot name an
//! executable, a PowerShell snippet or an extra argument. [`ElevationRequest::plan`]
//! validates every path and builds a fixed executable plus an argument array;
//! [`run`] then sends exactly one `Start-Process -Verb RunAs` request through a
//! hidden PowerShell helper and maps the result to
//! `completed`/`cancelled`/`failed`/`unsupported`.
//!
//! Safety rules encoded here:
//! - only the operations below exist; unknown names fail to deserialize;
//! - paths must be absolute, free of `..`, and resolve inside the expected root
//!   (`Program Files` for ACLs/services/config files, Ludex's verified download
//!   cache for installers);
//! - arguments are passed as arrays (or as a UTF-16LE `-EncodedCommand`), never
//!   interpolated into a shell string;
//! - one prompt at a time, never persisted, never automatic, and the elevated
//!   process inherits the user's identity (no service, no token, no database);
//! - non-Windows platforms return a typed `unsupported` result.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::process;
use crate::streaming::{path_under_roots, program_files_roots};

/// Longest `reason` accepted; it is shown in the confirmation dialog.
const MAX_REASON_CHARS: usize = 400;
/// Longest extra installer argument accepted.
const MAX_INSTALLER_ARGUMENT_CHARS: usize = 512;
/// Largest config file the elevated helper writes in one request. The payload
/// travels base64-encoded inside two nested `-EncodedCommand` scripts, so the
/// cap keeps the final Windows command line well below 32 KiB.
const MAX_CONFIG_BYTES: usize = 2 * 1024;
/// Extensions a config write may target: the operation must not replace an
/// executable or a driver with arbitrary bytes.
const CONFIG_EXTENSIONS: [&str; 8] = ["conf", "json", "ini", "cfg", "txt", "xml", "yaml", "yml"];
/// A privileged operation that outlives this budget is killed.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// `ERROR_CANCELLED`: the user declined the UAC prompt.
pub const UAC_CANCELLED: i32 = 1223;
/// `ERROR_TIMEOUT`: the helper's own budget expired.
pub const ELEVATION_TIMEOUT: i32 = 1460;

static PROMPT_OPEN: AtomicBool = AtomicBool::new(false);

/// Whitelisted privileged operations. Each variant fixes the executable and
/// the shape of its arguments; no caller can run an arbitrary command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ElevationOperation {
    /// `icacls "<dir>" /grant "<user>:(OI)(CI)M" /Q` on an existing folder.
    GrantPathAcl,
    /// A verified `.msi` (through `msiexec`) or `.exe` installer from Ludex's
    /// download cache, with the catalog's fixed argument list.
    RunInstaller,
    /// `sc.exe create <name> binPath= <binary>`.
    InstallService,
    /// `sc.exe create <name> type= kernel binPath= <driver>`.
    InstallDriver,
    /// `netsh advfirewall firewall add rule` for a `Ludex`-prefixed rule.
    AddFirewallRule,
    /// `netsh advfirewall firewall delete rule` for a `Ludex`-prefixed rule.
    RemoveFirewallRule,
    /// Replace one configuration file under `Program Files`.
    WriteConfigFile,
}

impl ElevationOperation {
    /// Every whitelisted operation; the web UI mirrors this list and the tests
    /// assert that each one plans.
    #[allow(dead_code)]
    pub const ALL: [Self; 7] = [
        Self::GrantPathAcl,
        Self::RunInstaller,
        Self::InstallService,
        Self::InstallDriver,
        Self::AddFirewallRule,
        Self::RemoveFirewallRule,
        Self::WriteConfigFile,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::GrantPathAcl => "grantPathAcl",
            Self::RunInstaller => "runInstaller",
            Self::InstallService => "installService",
            Self::InstallDriver => "installDriver",
            Self::AddFirewallRule => "addFirewallRule",
            Self::RemoveFirewallRule => "removeFirewallRule",
            Self::WriteConfigFile => "writeConfigFile",
        }
    }

    /// Plain-language label for logs and the confirmation dialog.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::GrantPathAcl => "Grant modify rights on a folder",
            Self::RunInstaller => "Run a verified installer",
            Self::InstallService => "Install a Windows service",
            Self::InstallDriver => "Install a driver",
            Self::AddFirewallRule => "Add a firewall rule",
            Self::RemoveFirewallRule => "Remove a firewall rule",
            Self::WriteConfigFile => "Write a configuration file",
        }
    }
}

/// Folders a privileged operation may touch. Callers build this from the
/// environment (or from the verified download cache) and never from webview
/// input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ElevationRoots {
    /// `Program Files` and `Program Files (x86)`.
    pub program_files: Vec<PathBuf>,
    /// Ludex's runtime download cache: only artifacts whose size and SHA-256
    /// matched the signed manifest live here.
    pub installers: Vec<PathBuf>,
}

impl ElevationRoots {
    /// Windows roots from the current environment.
    pub fn from_env() -> Self {
        let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        Self {
            program_files: program_files_roots(&env),
            installers: Vec::new(),
        }
    }

    pub fn with_installers(mut self, installers: Vec<PathBuf>) -> Self {
        self.installers = installers;
        self
    }
}

/// The typed elevation request: a whitelisted operation, its arguments and the
/// sentence the confirmation dialog shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevationRequest {
    pub operation: ElevationOperation,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub reason: String,
}

/// One argument of the elevated command. The current-identity variants are
/// rendered as fixed PowerShell expressions, never as caller input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElevatedArgument {
    /// A literal token; quoted for the Windows command line when it contains a
    /// space. `"` and control characters are rejected before this point.
    Literal(String),
    /// `[System.Security.Principal.WindowsIdentity]::GetCurrent().Name`.
    CurrentUser,
    /// The current identity with `:(OI)(CI)M` appended.
    CurrentUserModify,
}

/// A fully validated privileged command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElevatedCommand {
    /// Fixed executable, chosen by the operation.
    pub program: PathBuf,
    /// Argument array handed to `Start-Process -ArgumentList`.
    pub arguments: Vec<ElevatedArgument>,
    /// Canonical path or rule the dialog and the audit log name.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ElevationStatus {
    Completed,
    Cancelled,
    Failed,
    Unsupported,
}

impl ElevationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Outcome of one elevation request. `exit_code` is diagnostic only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElevationResult {
    pub status: ElevationStatus,
    pub operation: ElevationOperation,
    pub target: String,
    pub message: String,
    pub elapsed_ms: u64,
    pub exit_code: Option<i32>,
}

/// True on the only platform with UAC.
pub fn supported() -> bool {
    cfg!(windows)
}

/* ------------------------------ validation ------------------------------- */

fn single_argument<'a>(request: &'a ElevationRequest, operation: &str) -> Result<&'a str, String> {
    match request.arguments.as_slice() {
        [value] => Ok(value.as_str()),
        _ => Err(format!(
            "{operation} takes exactly one argument, got {}.",
            request.arguments.len()
        )),
    }
}

fn validate_reason(reason: &str) -> Result<(), String> {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return Err("An elevation request needs a reason.".to_string());
    }
    if trimmed.chars().count() > MAX_REASON_CHARS {
        return Err(format!(
            "The elevation reason is longer than {MAX_REASON_CHARS} characters."
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err("The elevation reason contains a control character.".to_string());
    }
    Ok(())
}

/// Reject the characters Windows cannot store in a path and the ones that
/// would break the generated argument list.
fn validate_path_text(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("The path is empty.".to_string());
    }
    if path
        .chars()
        .any(|ch| ch == '"' || ch == '\0' || ch == '\n' || ch == '\r' || ch.is_control())
    {
        return Err(format!(
            "The path contains an unsupported character: {path}"
        ));
    }
    if path.contains('*') || path.contains('?') {
        return Err(format!("The path contains a wildcard: {path}"));
    }
    Ok(())
}

/// True when `path` contains a `..` or `.` component. The lexical check runs
/// before any filesystem call so a traversal is refused even when the target
/// does not exist.
fn has_traversal(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
}

/// Canonicalise `dir` and accept it only below one of `roots`. The canonical
/// form also rejects `..` traversals and symlinks that leave the root.
pub fn validate_dir_in_roots(
    dir: &Path,
    roots: &[PathBuf],
    description: &str,
) -> Result<PathBuf, String> {
    validate_path_text(&dir.to_string_lossy())?;
    if has_traversal(dir) {
        return Err(format!(
            "Refusing to use {description} {}: the path contains '..'.",
            dir.display()
        ));
    }
    if !dir.is_absolute() {
        return Err(format!(
            "Refusing to use {description} {}: the path is not absolute.",
            dir.display()
        ));
    }
    if roots.is_empty() {
        return Err(format!(
            "Refusing to use {description} {}: no trusted root is configured.",
            dir.display()
        ));
    }
    let canonical = std::fs::canonicalize(dir).map_err(|error| {
        format!(
            "Could not resolve {description} {}: {error}.",
            dir.display()
        )
    })?;
    let canonical = paths::normalize_native_path(&canonical);
    if path_under_roots(&canonical, roots) {
        Ok(canonical)
    } else {
        Err(format!(
            "Refusing to use {description} {}: it is outside the expected folders.",
            canonical.display()
        ))
    }
}

/// Like [`validate_dir_in_roots`], for a file that must already exist.
fn validate_file_in_roots(
    file: &Path,
    roots: &[PathBuf],
    description: &str,
) -> Result<PathBuf, String> {
    validate_path_text(&file.to_string_lossy())?;
    if has_traversal(file) {
        return Err(format!(
            "Refusing to use {description} {}: the path contains '..'.",
            file.display()
        ));
    }
    if !file.is_absolute() {
        return Err(format!(
            "Refusing to use {description} {}: the path is not absolute.",
            file.display()
        ));
    }
    if roots.is_empty() {
        return Err(format!(
            "Refusing to use {description} {}: no trusted root is configured.",
            file.display()
        ));
    }
    if !file.is_file() {
        return Err(format!("{description} {} does not exist.", file.display()));
    }
    let canonical = std::fs::canonicalize(file).map_err(|error| {
        format!(
            "Could not resolve {description} {}: {error}.",
            file.display()
        )
    })?;
    let canonical = paths::normalize_native_path(&canonical);
    if path_under_roots(&canonical, roots) {
        Ok(canonical)
    } else {
        Err(format!(
            "Refusing to use {description} {}: it is outside the expected folders.",
            canonical.display()
        ))
    }
}

/// Validate a file the operation will create: the parent folder must already
/// exist under a trusted root, and the file name must be a single component.
fn validate_new_file_in_roots(
    file: &Path,
    roots: &[PathBuf],
    description: &str,
) -> Result<PathBuf, String> {
    validate_path_text(&file.to_string_lossy())?;
    if has_traversal(file) || !file.is_absolute() {
        return Err(format!(
            "Refusing to write {description} {}: the path must be absolute and free of '..'.",
            file.display()
        ));
    }
    let name = file
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("{description} has no file name: {}.", file.display()))?;
    let name_text = name.to_string_lossy();
    if name_text
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '|' | '/' | '\\'))
    {
        return Err(format!(
            "Refusing to write {description}: the file name contains an unsupported character."
        ));
    }
    let parent = file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| format!("{description} has no folder: {}.", file.display()))?;
    let parent = validate_dir_in_roots(parent, roots, description)?;
    Ok(parent.join(name))
}

fn validate_installer_argument(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value {
        return Err("An installer argument is empty or has surrounding spaces.".to_string());
    }
    if value.chars().count() > MAX_INSTALLER_ARGUMENT_CHARS {
        return Err(format!(
            "An installer argument is longer than {MAX_INSTALLER_ARGUMENT_CHARS} characters."
        ));
    }
    // Fixed catalog arguments only: no quoting, expansion or shell syntax.
    let allowed = |ch: char| {
        ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '.' | '/' | ':' | '=' | '+' | '\\' | '-')
    };
    if !value.chars().all(allowed) {
        return Err(format!(
            "The installer argument {value} contains an unsupported character."
        ));
    }
    Ok(())
}

fn validate_service_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        && name
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphanumeric());
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Refusing the service name {name}: use letters, digits, '.', '_' or '-' (max 64)."
        ))
    }
}

fn validate_rule_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.starts_with("Ludex")
        && name.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '.' | '_' | '(' | ')' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Refusing the firewall rule {name}: only Ludex-owned rules (prefix \"Ludex\") can be changed."
        ))
    }
}

fn validate_protocol(value: &str) -> Result<String, String> {
    match value.to_ascii_lowercase().as_str() {
        "tcp" => Ok("TCP".to_string()),
        "udp" => Ok("UDP".to_string()),
        _ => Err(format!(
            "Refusing the firewall protocol {value}: only TCP and UDP are supported."
        )),
    }
}

fn validate_direction(value: &str) -> Result<&'static str, String> {
    match value.to_ascii_lowercase().as_str() {
        "in" => Ok("in"),
        "out" => Ok("out"),
        _ => Err(format!(
            "Refusing the firewall direction {value}: use \"in\" or \"out\"."
        )),
    }
}

/// `47984` or `47984-48010`, each port in 1..=65535.
fn validate_ports(value: &str) -> Result<String, String> {
    let parse = |part: &str| -> Result<u16, String> {
        part.parse::<u16>()
            .map_err(|_| format!("Refusing the firewall port {value}: use a port or a port range."))
    };
    let (start, end) = match value.split_once('-') {
        Some((start, end)) => (parse(start)?, parse(end)?),
        None => {
            let port = parse(value)?;
            (port, port)
        }
    };
    if start == 0 || end == 0 || end < start {
        return Err(format!(
            "Refusing the firewall port range {value}: invalid bounds."
        ));
    }
    Ok(if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    })
}

/* ---------------------------- argument builder --------------------------- */

/// Escape a PowerShell single-quoted string literal.
fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Quote one Windows command-line token when it contains a space. Literal
/// values with `"` never reach this point (path and argument validation).
fn windows_token(value: &str) -> String {
    if value.contains(' ') {
        format!("\"{value}\"")
    } else {
        value.to_string()
    }
}

fn path_text(path: &Path) -> String {
    paths::normalize_native_path(path)
        .to_string_lossy()
        .to_string()
}

impl ElevationRequest {
    /// Best-effort target for error results before `plan` succeeds.
    pub fn target_hint(&self) -> String {
        self.arguments
            .first()
            .cloned()
            .unwrap_or_else(|| self.operation.as_str().to_string())
    }

    /// Validate the request and build the fixed elevated command. Pure apart
    /// from `canonicalize`/`is_file` on the paths, so every branch is testable
    /// without UAC.
    pub fn plan(&self, roots: &ElevationRoots) -> Result<ElevatedCommand, String> {
        validate_reason(&self.reason)?;
        match self.operation {
            ElevationOperation::GrantPathAcl => {
                let dir = single_argument(self, "grantPathAcl")?;
                let target =
                    validate_dir_in_roots(Path::new(dir), &roots.program_files, "the folder")?;
                Ok(ElevatedCommand {
                    program: PathBuf::from("icacls.exe"),
                    arguments: vec![
                        ElevatedArgument::Literal(path_text(&target)),
                        ElevatedArgument::Literal("/grant".to_string()),
                        ElevatedArgument::CurrentUserModify,
                        ElevatedArgument::Literal("/Q".to_string()),
                    ],
                    target: path_text(&target),
                })
            }
            ElevationOperation::RunInstaller => {
                let installer = self.arguments.first().ok_or_else(|| {
                    "runInstaller needs the installer path as its first argument.".to_string()
                })?;
                let installer = validate_file_in_roots(
                    Path::new(installer),
                    &roots.installers,
                    "the installer",
                )?;
                let extension = installer
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let (program, mut arguments) = match extension.as_str() {
                    "msi" => (
                        PathBuf::from("msiexec.exe"),
                        vec![
                            ElevatedArgument::Literal("/i".to_string()),
                            ElevatedArgument::Literal(path_text(&installer)),
                        ],
                    ),
                    // The executable is the elevated program itself; only the
                    // catalog arguments follow.
                    "exe" => (installer.clone(), Vec::new()),
                    _ => {
                        return Err(format!(
                            "Refusing to elevate {}: only .msi and .exe installers are supported.",
                            installer.display()
                        ))
                    }
                };
                for extra in self.arguments.iter().skip(1) {
                    validate_installer_argument(extra)?;
                    arguments.push(ElevatedArgument::Literal(extra.clone()));
                }
                Ok(ElevatedCommand {
                    program,
                    arguments,
                    target: path_text(&installer),
                })
            }
            ElevationOperation::InstallService | ElevationOperation::InstallDriver => {
                let driver = self.operation == ElevationOperation::InstallDriver;
                let (name, binary) = match self.arguments.as_slice() {
                    [name, binary] => (name, binary),
                    _ => {
                        return Err(format!(
                            "{} takes a service name and a binary path.",
                            self.operation.as_str()
                        ))
                    }
                };
                validate_service_name(name)?;
                let binary = validate_file_in_roots(
                    Path::new(binary),
                    &roots.program_files,
                    "the service binary",
                )?;
                if driver
                    && binary
                        .extension()
                        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                        != Some("sys".to_string())
                {
                    return Err(format!(
                        "Refusing to install {}: a driver must be a .sys file.",
                        binary.display()
                    ));
                }
                let mut arguments = vec![
                    ElevatedArgument::Literal("create".to_string()),
                    ElevatedArgument::Literal(name.clone()),
                ];
                if driver {
                    arguments.push(ElevatedArgument::Literal("type=".to_string()));
                    arguments.push(ElevatedArgument::Literal("kernel".to_string()));
                }
                arguments.push(ElevatedArgument::Literal("binPath=".to_string()));
                arguments.push(ElevatedArgument::Literal(path_text(&binary)));
                Ok(ElevatedCommand {
                    program: PathBuf::from("sc.exe"),
                    arguments,
                    target: path_text(&binary),
                })
            }
            ElevationOperation::AddFirewallRule => {
                let (name, protocol, ports, direction) = match self.arguments.as_slice() {
                    [name, protocol, ports, direction] => (name, protocol, ports, direction),
                    _ => {
                        return Err(
                            "addFirewallRule takes a name, a protocol, ports and a direction."
                                .to_string(),
                        )
                    }
                };
                validate_rule_name(name)?;
                let protocol = validate_protocol(protocol)?;
                let ports = validate_ports(ports)?;
                let direction = validate_direction(direction)?;
                let port_key = if direction == "in" {
                    "localport="
                } else {
                    "remoteport="
                };
                Ok(ElevatedCommand {
                    program: PathBuf::from("netsh.exe"),
                    arguments: vec![
                        ElevatedArgument::Literal("advfirewall".to_string()),
                        ElevatedArgument::Literal("firewall".to_string()),
                        ElevatedArgument::Literal("add".to_string()),
                        ElevatedArgument::Literal("rule".to_string()),
                        ElevatedArgument::Literal(format!("name={name}")),
                        ElevatedArgument::Literal(format!("dir={direction}")),
                        ElevatedArgument::Literal("action=allow".to_string()),
                        ElevatedArgument::Literal(format!("protocol={protocol}")),
                        ElevatedArgument::Literal(format!("{port_key}{ports}")),
                    ],
                    target: format!("{name} ({protocol} {ports} {direction})"),
                })
            }
            ElevationOperation::RemoveFirewallRule => {
                let name = single_argument(self, "removeFirewallRule")?;
                validate_rule_name(name)?;
                Ok(ElevatedCommand {
                    program: PathBuf::from("netsh.exe"),
                    arguments: vec![
                        ElevatedArgument::Literal("advfirewall".to_string()),
                        ElevatedArgument::Literal("firewall".to_string()),
                        ElevatedArgument::Literal("delete".to_string()),
                        ElevatedArgument::Literal("rule".to_string()),
                        ElevatedArgument::Literal(format!("name={name}")),
                    ],
                    target: name.to_string(),
                })
            }
            ElevationOperation::WriteConfigFile => {
                let (path, payload) = match self.arguments.as_slice() {
                    [path, payload] => (path, payload),
                    _ => {
                        return Err("writeConfigFile takes a path and the base64 file contents."
                            .to_string())
                    }
                };
                let target = validate_new_file_in_roots(
                    Path::new(path),
                    &roots.program_files,
                    "the configuration file",
                )?;
                let extension = target
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if !CONFIG_EXTENSIONS.contains(&extension.as_str()) {
                    return Err(format!(
                        "Refusing to write {}: only configuration files are allowed.",
                        target.display()
                    ));
                }
                let bytes = base64_decode(payload)?;
                if bytes.len() > MAX_CONFIG_BYTES {
                    return Err(format!(
                        "Refusing to write {}: the file is larger than {MAX_CONFIG_BYTES} bytes.",
                        target.display()
                    ));
                }
                let script = write_file_script(&target, &bytes);
                let encoded = encode_powershell(&script);
                Ok(ElevatedCommand {
                    program: PathBuf::from("powershell.exe"),
                    arguments: vec![
                        ElevatedArgument::Literal("-NoProfile".to_string()),
                        ElevatedArgument::Literal("-NonInteractive".to_string()),
                        ElevatedArgument::Literal("-EncodedCommand".to_string()),
                        ElevatedArgument::Literal(encoded),
                    ],
                    target: path_text(&target),
                })
            }
        }
    }
}

/* ------------------------------ PowerShell ------------------------------- */

/// The hidden non-elevated helper: one `Start-Process -Verb RunAs`, bounded by
/// `timeout`, whose exit code becomes ours.
pub fn elevation_script(command: &ElevatedCommand, timeout: Duration) -> String {
    let mut arguments = Vec::with_capacity(command.arguments.len());
    let mut needs_user = false;
    for argument in &command.arguments {
        match argument {
            ElevatedArgument::Literal(value) => {
                arguments.push(ps_literal(&windows_token(value)));
            }
            ElevatedArgument::CurrentUser => {
                needs_user = true;
                arguments.push("$user".to_string());
            }
            ElevatedArgument::CurrentUserModify => {
                needs_user = true;
                arguments.push("($user + ':(OI)(CI)M')".to_string());
            }
        }
    }
    let user = if needs_user {
        "$user = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name\n"
    } else {
        ""
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128);
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         {user}\
         $timeoutMs = {timeout_ms}\n\
         try {{\n\
         \x20 $arguments = @({arguments})\n\
         \x20 $start = @{{ FilePath = {program}; Verb = 'RunAs'; PassThru = $true }}\n\
         \x20 if ($arguments.Count -gt 0) {{ $start.ArgumentList = $arguments }}\n\
         \x20 $process = Start-Process @start\n\
         \x20 if (-not $process.WaitForExit($timeoutMs)) {{\n\
         \x20   try {{ $process.Kill() }} catch {{ }}\n\
         \x20   [Console]::Error.WriteLine('The elevated operation timed out.')\n\
         \x20   exit {timeout_code}\n\
         \x20 }}\n\
         \x20 exit $process.ExitCode\n\
         }} catch {{\n\
         \x20 if ($_.Exception.HResult -eq -2147023673) {{ exit {cancelled} }}\n\
         \x20 [Console]::Error.WriteLine($_.Exception.Message)\n\
         \x20 exit 1\n\
         }}\n",
        arguments = arguments.join(", "),
        program = ps_literal(&path_text(&command.program)),
        timeout_code = ELEVATION_TIMEOUT,
        cancelled = UAC_CANCELLED,
    )
}

fn write_file_script(target: &Path, bytes: &[u8]) -> String {
    let payload = base64_encode(bytes);
    format!(
        "$ErrorActionPreference = 'Stop'\n$bytes = [Convert]::FromBase64String({payload})\n[System.IO.File]::WriteAllBytes({path}, $bytes)\n",
        payload = ps_literal(&payload),
        path = ps_literal(&path_text(target)),
    )
}

/// Encode a PowerShell script for `-EncodedCommand`: UTF-16LE, base64.
pub fn encode_powershell(script: &str) -> String {
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&bytes)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
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

/// Strict base64 decode used to bound the config payload. Standard alphabet,
/// padded length, no whitespace.
fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
    fn sextet(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
            b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let invalid = || "The file contents are not valid base64.".to_string();
    if value.is_empty() {
        return Ok(Vec::new());
    }
    if !value.len().is_multiple_of(4) {
        return Err(invalid());
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(value.len() / 4 * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index + 1 == bytes.len() / 4;
        let padding = match (chunk[2], chunk[3]) {
            (b'=', b'=') => 2,
            (_, b'=') => 1,
            _ => 0,
        };
        if padding > 0 && !last {
            return Err(invalid());
        }
        let mut quadruple = 0u32;
        for (offset, byte) in chunk.iter().enumerate() {
            let value = if *byte == b'=' {
                if offset < 4 - padding {
                    return Err(invalid());
                }
                0
            } else {
                sextet(*byte).ok_or_else(invalid)?
            };
            quadruple = (quadruple << 6) | value;
        }
        out.push(((quadruple >> 16) & 0xff) as u8);
        if padding < 2 {
            out.push(((quadruple >> 8) & 0xff) as u8);
        }
        if padding == 0 {
            out.push((quadruple & 0xff) as u8);
        }
    }
    Ok(out)
}

/* -------------------------------- running -------------------------------- */

/// Map the helper's exit code to a typed status. Pure so the mapping is
/// testable without running UAC.
pub fn map_exit_code(exit_code: Option<i32>, detail: &str) -> (ElevationStatus, String) {
    match exit_code {
        Some(UAC_CANCELLED) => (
            ElevationStatus::Cancelled,
            "The elevation prompt was cancelled; nothing was changed.".to_string(),
        ),
        Some(0) => (
            ElevationStatus::Completed,
            "The administrator operation completed.".to_string(),
        ),
        Some(ELEVATION_TIMEOUT) => (
            ElevationStatus::Failed,
            "The elevated operation timed out before it finished.".to_string(),
        ),
        Some(code) => (
            ElevationStatus::Failed,
            format!("Windows reported exit code {code}: {}", detail_line(detail)),
        ),
        None => (
            ElevationStatus::Failed,
            format!(
                "The elevation helper did not report an exit code: {}",
                detail_line(detail)
            ),
        ),
    }
}

fn detail_line(detail: &str) -> String {
    let line = detail
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no detail");
    line.chars().take(300).collect()
}

struct PromptGuard;

impl Drop for PromptGuard {
    fn drop(&mut self) {
        PROMPT_OPEN.store(false, Ordering::SeqCst);
    }
}

/// Run one elevation request. Only the whitelisted operation runs elevated;
/// Ludex stays at the user's integrity level and no token or database is read
/// by the helper.
pub fn run(request: &ElevationRequest, roots: &ElevationRoots) -> ElevationResult {
    run_with_timeout(request, roots, DEFAULT_TIMEOUT)
}

pub fn run_with_timeout(
    request: &ElevationRequest,
    roots: &ElevationRoots,
    timeout: Duration,
) -> ElevationResult {
    let started = Instant::now();
    if !supported() {
        let message = "Elevation is only available on Windows; nothing was changed.".to_string();
        tracing::info!(
            operation = request.operation.as_str(),
            target = %request.target_hint(),
            status = ElevationStatus::Unsupported.as_str(),
            "elevation.unsupported"
        );
        return ElevationResult {
            status: ElevationStatus::Unsupported,
            operation: request.operation,
            target: request.target_hint(),
            message,
            elapsed_ms: 0,
            exit_code: None,
        };
    }
    let command = match request.plan(roots) {
        Ok(command) => command,
        Err(message) => {
            tracing::warn!(
                operation = request.operation.as_str(),
                target = %request.target_hint(),
                status = ElevationStatus::Failed.as_str(),
                "elevation.failed"
            );
            return ElevationResult {
                status: ElevationStatus::Failed,
                operation: request.operation,
                target: request.target_hint(),
                message,
                elapsed_ms: started.elapsed().as_millis() as u64,
                exit_code: None,
            };
        }
    };
    if PROMPT_OPEN.swap(true, Ordering::SeqCst) {
        let message =
            "Another administrator prompt is already open; finish or cancel it first.".to_string();
        tracing::warn!(
            operation = request.operation.as_str(),
            target = %command.target,
            status = ElevationStatus::Failed.as_str(),
            "elevation.busy"
        );
        return ElevationResult {
            status: ElevationStatus::Failed,
            operation: request.operation,
            target: command.target,
            message,
            elapsed_ms: started.elapsed().as_millis() as u64,
            exit_code: None,
        };
    }
    let _guard = PromptGuard;
    tracing::info!(
        operation = request.operation.as_str(),
        target = %command.target,
        "elevation.requested"
    );
    let script = elevation_script(&command, timeout);
    let encoded = encode_powershell(&script);
    let output = process::capture("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .output();
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let (status, message, exit_code) = match output {
        Ok(output) => {
            let mut detail = String::from_utf8_lossy(&output.stderr).to_string();
            if detail.trim().is_empty() {
                detail = String::from_utf8_lossy(&output.stdout).to_string();
            }
            let code = output.status.code();
            let (status, message) = map_exit_code(code, &detail);
            (status, message, code)
        }
        Err(error) => (
            ElevationStatus::Failed,
            format!("Could not start the elevation helper: {error}."),
            None,
        ),
    };
    match status {
        ElevationStatus::Completed => tracing::info!(
            operation = request.operation.as_str(),
            target = %command.target,
            elapsed_ms,
            "elevation.granted"
        ),
        ElevationStatus::Cancelled => tracing::info!(
            operation = request.operation.as_str(),
            target = %command.target,
            elapsed_ms,
            "elevation.cancelled"
        ),
        _ => tracing::warn!(
            operation = request.operation.as_str(),
            target = %command.target,
            elapsed_ms,
            exit_code = ?exit_code,
            "elevation.failed"
        ),
    }
    ElevationResult {
        status,
        operation: request.operation,
        target: command.target,
        message,
        elapsed_ms,
        exit_code,
    }
}

/* --------------------------- reusable helpers ---------------------------- */

/// #182: grant the current Windows account modify rights on a detected
/// Sunshine config folder.
pub fn grant_path_acl(dir: &Path, roots: &ElevationRoots) -> ElevationResult {
    run(
        &ElevationRequest {
            operation: ElevationOperation::GrantPathAcl,
            arguments: vec![dir.to_string_lossy().to_string()],
            reason:
                "Windows blocks writes to this program folder; the current account needs modify rights."
                    .to_string(),
        },
        roots,
    )
}

/// #178: run a verified installer from Ludex's download cache.
pub fn run_verified_installer(
    installer: &Path,
    arguments: &[String],
    roots: &ElevationRoots,
) -> ElevationResult {
    let mut request_arguments = vec![installer.to_string_lossy().to_string()];
    request_arguments.extend(arguments.iter().cloned());
    run(
        &ElevationRequest {
            operation: ElevationOperation::RunInstaller,
            arguments: request_arguments,
            reason: "The machine-wide prerequisite installer needs administrator rights."
                .to_string(),
        },
        roots,
    )
}

/// #179: add a Ludex-owned Windows Firewall rule.
pub fn add_firewall_rule(
    name: &str,
    protocol: &str,
    ports: &str,
    direction: &str,
    roots: &ElevationRoots,
) -> ElevationResult {
    run(
        &ElevationRequest {
            operation: ElevationOperation::AddFirewallRule,
            arguments: vec![
                name.to_string(),
                protocol.to_string(),
                ports.to_string(),
                direction.to_string(),
            ],
            reason: "Sunshine needs a Windows Firewall allowance for remote play.".to_string(),
        },
        roots,
    )
}

/// #179: remove a Ludex-owned Windows Firewall rule.
pub fn remove_firewall_rule(name: &str, roots: &ElevationRoots) -> ElevationResult {
    run(
        &ElevationRequest {
            operation: ElevationOperation::RemoveFirewallRule,
            arguments: vec![name.to_string()],
            reason: "Ludex is removing the firewall allowance it added.".to_string(),
        },
        roots,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-elevate-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(
        operation: ElevationOperation,
        arguments: Vec<&str>,
        reason: &str,
    ) -> ElevationRequest {
        ElevationRequest {
            operation,
            arguments: arguments.into_iter().map(str::to_string).collect(),
            reason: reason.to_string(),
        }
    }

    #[test]
    fn only_whitelisted_operations_deserialize() {
        for operation in ElevationOperation::ALL {
            let request: ElevationRequest = serde_json::from_value(json!({
                "operation": operation.as_str(),
                "arguments": [],
                "reason": "test",
            }))
            .unwrap();
            assert_eq!(request.operation, operation);
        }
        let unknown = serde_json::from_value::<ElevationRequest>(json!({
            "operation": "runArbitraryCommand",
            "arguments": ["cmd.exe"],
            "reason": "test",
        }));
        assert!(unknown.is_err());
        assert_eq!(ElevationOperation::ALL.len(), 7);
        for operation in ElevationOperation::ALL {
            assert!(!operation.label().is_empty());
        }
    }

    #[test]
    fn a_request_needs_a_short_control_free_reason() {
        let root = temp_dir("reason");
        let roots = ElevationRoots {
            program_files: vec![root.clone()],
            installers: Vec::new(),
        };
        let dir = root.join("Sunshine");
        std::fs::create_dir_all(&dir).unwrap();
        let args = vec![dir.to_string_lossy().to_string()];
        let long_reason = "x".repeat(MAX_REASON_CHARS + 1);

        for reason in ["", "   ", long_reason.as_str()] {
            let request = ElevationRequest {
                operation: ElevationOperation::GrantPathAcl,
                arguments: args.clone(),
                reason: reason.to_string(),
            };
            assert!(request.plan(&roots).unwrap_err().contains("reason"));
        }
        let newline = ElevationRequest {
            operation: ElevationOperation::GrantPathAcl,
            arguments: args.clone(),
            reason: "line\nbreak".to_string(),
        };
        assert!(newline.plan(&roots).is_err());
        let ok = ElevationRequest {
            operation: ElevationOperation::GrantPathAcl,
            arguments: args,
            reason: "Sunshine writes its config under Program Files.".to_string(),
        };
        assert!(ok.plan(&roots).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_acl_plan_matches_the_182_command_contract() {
        let root = temp_dir("acl");
        let program_files = root.join("Program Files");
        let config = program_files.join("Sunshine").join("config");
        std::fs::create_dir_all(&config).unwrap();
        let roots = ElevationRoots {
            program_files: vec![program_files.clone()],
            installers: Vec::new(),
        };
        let request = request(
            ElevationOperation::GrantPathAcl,
            vec![&config.to_string_lossy()],
            "Windows blocks Sunshine's config folder.",
        );
        let command = request.plan(&roots).unwrap();
        assert_eq!(command.program, PathBuf::from("icacls.exe"));
        assert_eq!(
            command.arguments[1],
            ElevatedArgument::Literal("/grant".to_string())
        );
        assert_eq!(command.arguments[2], ElevatedArgument::CurrentUserModify);
        assert_eq!(
            command.arguments[3],
            ElevatedArgument::Literal("/Q".to_string())
        );
        // The folder token is the raw canonical path; the script quotes it for
        // the command line because it contains a space.
        let ElevatedArgument::Literal(token) = &command.arguments[0] else {
            panic!("expected a literal folder token");
        };
        assert!(!token.contains('"'));
        assert!(token.contains("Sunshine"));
        assert_eq!(
            command.target,
            std::fs::canonicalize(&config)
                .unwrap()
                .to_string_lossy()
                .to_string()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installer_plans_use_msiexec_for_an_msi_and_the_exe_itself_otherwise() {
        let root = temp_dir("installer");
        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let msi = cache.join("vc_redist.x64.msi");
        let exe = cache.join("DirectX setup.exe");
        std::fs::write(&msi, b"msi").unwrap();
        std::fs::write(&exe, b"exe").unwrap();
        let roots = ElevationRoots {
            program_files: Vec::new(),
            installers: vec![cache.clone()],
        };

        let command = request(
            ElevationOperation::RunInstaller,
            vec![&msi.to_string_lossy(), "/quiet", "/norestart"],
            "A redistributable needs administrator rights.",
        )
        .plan(&roots)
        .unwrap();
        assert_eq!(command.program, PathBuf::from("msiexec.exe"));
        assert_eq!(
            command.arguments[0],
            ElevatedArgument::Literal("/i".to_string())
        );
        let ElevatedArgument::Literal(path) = &command.arguments[1] else {
            panic!("expected the msi path");
        };
        assert!(!path.contains('"'));
        assert!(path.ends_with("vc_redist.x64.msi"));

        let command = request(
            ElevationOperation::RunInstaller,
            vec![&exe.to_string_lossy(), "/quiet"],
            "DirectX needs administrator rights.",
        )
        .plan(&roots)
        .unwrap();
        assert_eq!(command.program, exe);
        assert_eq!(
            command.arguments,
            vec![ElevatedArgument::Literal("/quiet".to_string())]
        );
        assert!(command.target.contains("DirectX setup.exe"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installer_arguments_cannot_smuggle_a_second_command() {
        let args = [
            "\"; rm -rf /",
            "a\nb",
            " a",
            "$(calc)",
            "`calc`",
            "&calc",
            "a\0b",
        ];
        for value in args {
            assert!(
                validate_installer_argument(value).is_err(),
                "{value} must be refused"
            );
        }
        assert!(validate_installer_argument("/quiet /norestart").is_ok());
    }

    #[test]
    fn path_validation_rejects_relative_traversal_and_outside_roots() {
        let root = temp_dir("paths");
        let program_files = root.join("Program Files");
        let config = program_files.join("Sunshine").join("config");
        std::fs::create_dir_all(&config).unwrap();
        let outside = root.join("Games").join("Sunshine");
        std::fs::create_dir_all(&outside).unwrap();
        let roots = vec![program_files.clone()];

        let err = |value: &Path| validate_dir_in_roots(value, &roots, "the folder").unwrap_err();
        assert!(err(Path::new("Sunshine/config")).contains("absolute"));
        assert!(err(&config.join("..").join("..").join("Games")).contains("'..'"));
        assert!(err(&outside).contains("outside the expected folders"));
        assert!(err(&root.join("missing")).contains("Could not resolve"));
        // A root that is a prefix of another folder name is not containment.
        let sibling = root.join("Program FilesEvil");
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(err(&sibling).contains("outside the expected folders"));
        assert!(validate_dir_in_roots(&config, &roots, "the folder").is_ok());
        // Other architectures are separate roots, not children.
        let other_arch = root.join("Program Files (x86)").join("Sunshine");
        std::fs::create_dir_all(&other_arch).unwrap();
        assert!(err(&other_arch).contains("outside"));
        assert!(validate_dir_in_roots(
            &other_arch,
            &[root.join("Program Files (x86)")],
            "the folder"
        )
        .is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn symlinks_cannot_escape_the_expected_root() {
        #[cfg(unix)]
        {
            let root = temp_dir("symlink");
            let program_files = root.join("Program Files");
            std::fs::create_dir_all(&program_files).unwrap();
            let outside = root.join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            let link = program_files.join("Sunshine");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            let error = validate_dir_in_roots(&link, &[program_files], "the folder").unwrap_err();
            assert!(error.contains("outside the expected folders"));
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn service_and_driver_plans_use_sc_with_fixed_keywords() {
        let root = temp_dir("service");
        let program_files = root.join("Program Files");
        let binary = program_files.join("Ludex").join("ludex.exe");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"exe").unwrap();
        let driver = program_files.join("Ludex").join("ludex.sys");
        std::fs::write(&driver, b"sys").unwrap();
        let roots = ElevationRoots {
            program_files: vec![program_files.clone()],
            installers: Vec::new(),
        };

        let command = request(
            ElevationOperation::InstallService,
            vec!["LudexHelper", &binary.to_string_lossy()],
            "The service needs administrator rights.",
        )
        .plan(&roots)
        .unwrap();
        assert_eq!(command.program, PathBuf::from("sc.exe"));
        assert_eq!(
            command.arguments[0],
            ElevatedArgument::Literal("create".to_string())
        );
        assert!(command
            .arguments
            .contains(&ElevatedArgument::Literal("binPath=".to_string())));
        assert!(!command
            .arguments
            .contains(&ElevatedArgument::Literal("kernel".to_string())));

        let command = request(
            ElevationOperation::InstallDriver,
            vec!["LudexDriver", &driver.to_string_lossy()],
            "The driver needs administrator rights.",
        )
        .plan(&roots)
        .unwrap();
        assert!(command
            .arguments
            .contains(&ElevatedArgument::Literal("kernel".to_string())));

        // Names are a strict charset, and a driver must be a .sys file.
        assert!(request(
            ElevationOperation::InstallService,
            vec!["Ludex Helper; calc", &binary.to_string_lossy()],
            "reason"
        )
        .plan(&roots)
        .is_err());
        assert!(request(
            ElevationOperation::InstallDriver,
            vec!["LudexDriver", &binary.to_string_lossy()],
            "reason"
        )
        .plan(&roots)
        .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn firewall_plans_only_touch_ludex_rules() {
        let roots = ElevationRoots::default();
        let command = request(
            ElevationOperation::AddFirewallRule,
            vec!["Ludex remote play", "tcp", "47984-48010", "in"],
            "Sunshine needs a firewall allowance.",
        )
        .plan(&roots)
        .unwrap();
        assert_eq!(command.program, PathBuf::from("netsh.exe"));
        assert!(command.arguments.contains(&ElevatedArgument::Literal(
            "name=Ludex remote play".to_string()
        )));
        assert!(command
            .arguments
            .contains(&ElevatedArgument::Literal("protocol=TCP".to_string())));
        assert!(command.arguments.contains(&ElevatedArgument::Literal(
            "localport=47984-48010".to_string()
        )));

        let command = request(
            ElevationOperation::RemoveFirewallRule,
            vec!["Ludex remote play"],
            "Ludex is removing its rule.",
        )
        .plan(&roots)
        .unwrap();
        assert!(command.arguments.contains(&ElevatedArgument::Literal(
            "name=Ludex remote play".to_string()
        )));

        for name in ["Sunshine", "Ludex; calc", ""] {
            assert!(
                request(ElevationOperation::RemoveFirewallRule, vec![name], "reason")
                    .plan(&roots)
                    .is_err()
            );
        }
        for ports in ["0", "70000", "10-5", "http"] {
            assert!(request(
                ElevationOperation::AddFirewallRule,
                vec!["Ludex rule", "tcp", ports, "in"],
                "reason"
            )
            .plan(&roots)
            .is_err());
        }
        assert!(request(
            ElevationOperation::AddFirewallRule,
            vec!["Ludex rule", "icmp", "47984", "in"],
            "reason"
        )
        .plan(&roots)
        .is_err());
        assert!(request(
            ElevationOperation::AddFirewallRule,
            vec!["Ludex rule", "tcp", "47984", "sideways"],
            "reason"
        )
        .plan(&roots)
        .is_err());
    }

    #[test]
    fn config_writes_stay_small_and_inside_program_files() {
        let root = temp_dir("config");
        let program_files = root.join("Program Files");
        let config = program_files.join("Sunshine");
        std::fs::create_dir_all(&config).unwrap();
        let roots = ElevationRoots {
            program_files: vec![program_files.clone()],
            installers: Vec::new(),
        };
        let target = config.join("sunshine.conf");
        let payload = base64_encode(b"port = 47989\n");
        let command = request(
            ElevationOperation::WriteConfigFile,
            vec![&target.to_string_lossy(), &payload],
            "Sunshine's config file needs administrator rights.",
        )
        .plan(&roots)
        .unwrap();
        assert_eq!(command.program, PathBuf::from("powershell.exe"));
        assert!(command
            .arguments
            .contains(&ElevatedArgument::Literal("-EncodedCommand".to_string())));
        let ElevatedArgument::Literal(encoded) = command.arguments.last().expect("encoded command")
        else {
            panic!("expected the encoded command");
        };
        assert!(encoded.starts_with("JABFA"));

        // Too large: refused before any UAC prompt.
        let big = base64_encode(&vec![b'x'; MAX_CONFIG_BYTES + 1]);
        assert!(request(
            ElevationOperation::WriteConfigFile,
            vec![&target.to_string_lossy(), &big],
            "reason"
        )
        .plan(&roots)
        .is_err());
        // Not base64, or escaping the root, is refused too.
        assert!(request(
            ElevationOperation::WriteConfigFile,
            vec![&target.to_string_lossy(), "not base64!"],
            "reason"
        )
        .plan(&roots)
        .is_err());
        assert!(request(
            ElevationOperation::WriteConfigFile,
            vec![
                &root.join("Games").join("escape.conf").to_string_lossy(),
                &payload
            ],
            "reason"
        )
        .plan(&roots)
        .is_err());
        // The operation writes configuration files only, never executables or
        // alternate data streams.
        for name in ["ludex.exe", "apps.json:stream"] {
            assert!(request(
                ElevationOperation::WriteConfigFile,
                vec![&config.join(name).to_string_lossy(), &payload],
                "reason"
            )
            .plan(&roots)
            .is_err());
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_largest_config_write_fits_the_windows_command_line() {
        let root = temp_dir("config-line");
        let program_files = root.join("Program Files");
        let config = program_files.join("Sunshine");
        std::fs::create_dir_all(&config).unwrap();
        let roots = ElevationRoots {
            program_files: vec![program_files],
            installers: Vec::new(),
        };
        let payload = base64_encode(&vec![b'x'; MAX_CONFIG_BYTES]);
        let command = request(
            ElevationOperation::WriteConfigFile,
            vec![&config.join("sunshine.conf").to_string_lossy(), &payload],
            "Sunshine's config file needs administrator rights.",
        )
        .plan(&roots)
        .unwrap();
        let script = elevation_script(&command, DEFAULT_TIMEOUT);
        // The wrapper is passed through `-EncodedCommand`; Windows caps the
        // whole command line at 32767 characters, so keep a real margin.
        assert!(encode_powershell(&script).len() < 30_000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn base64_round_trips_and_rejects_malformed_input() {
        for bytes in [b"".as_slice(), b"A", b"AB", b"ABC", b"hello world"] {
            assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
        }
        for value in ["A", "AAA", "====", "AA=A", "not base64!"] {
            assert!(base64_decode(value).is_err(), "{value} must be refused");
        }
    }

    #[test]
    fn exit_codes_map_to_completed_cancelled_and_failed() {
        assert_eq!(map_exit_code(Some(0), "").0, ElevationStatus::Completed);
        let (status, message) = map_exit_code(Some(1223), "cancelled");
        assert_eq!(status, ElevationStatus::Cancelled);
        assert!(message.contains("nothing was changed"));
        let (status, message) = map_exit_code(Some(ELEVATION_TIMEOUT), "");
        assert_eq!(status, ElevationStatus::Failed);
        assert!(message.contains("timed out"));
        let (status, message) = map_exit_code(Some(5), "Access is denied.\nmore");
        assert_eq!(status, ElevationStatus::Failed);
        assert!(message.contains("5"));
        assert!(message.contains("Access is denied"));
        assert_eq!(map_exit_code(None, "").0, ElevationStatus::Failed);
    }

    #[test]
    fn the_script_is_one_elevated_process_with_a_bounded_wait() {
        let command = ElevatedCommand {
            program: PathBuf::from("icacls.exe"),
            arguments: vec![
                ElevatedArgument::Literal("C:\\Program Files\\Sunshine\\config".to_string()),
                ElevatedArgument::Literal("/grant".to_string()),
                ElevatedArgument::CurrentUserModify,
                ElevatedArgument::Literal("/Q".to_string()),
            ],
            target: "C:\\Program Files\\Sunshine\\config".to_string(),
        };
        let script = elevation_script(&command, Duration::from_secs(90));
        assert!(script.contains("Start-Process"));
        assert!(script.contains("Verb = 'RunAs'"));
        assert!(script.contains("PassThru = $true"));
        assert!(script.contains("WaitForExit($timeoutMs)"));
        assert!(script.contains("$timeoutMs = 90000"));
        assert!(script.contains("exit 1223"));
        assert!(script.contains("exit 1460"));
        assert!(script.contains("[System.Security.Principal.WindowsIdentity]::GetCurrent().Name"));
        assert!(script.contains("($user + ':(OI)(CI)M')"));
        assert!(script.contains(r#"'"C:\Program Files\Sunshine\config"'"#));
        // A single quote in a literal is doubled for PowerShell.
        let quoted = elevation_script(
            &ElevatedCommand {
                program: PathBuf::from("icacls.exe"),
                arguments: vec![ElevatedArgument::Literal(
                    "C:\\Program Files\\O'Brien".to_string(),
                )],
                target: String::new(),
            },
            Duration::from_secs(1),
        );
        assert!(quoted.contains("O''Brien"));
    }

    #[test]
    fn encoded_commands_are_utf16le_base64() {
        assert_eq!(encode_powershell("A"), "QQA=");
        assert_eq!(encode_powershell("AB"), "QQBCAA==");
        let command = ElevatedCommand {
            program: PathBuf::from("icacls.exe"),
            arguments: vec![ElevatedArgument::CurrentUser],
            target: String::new(),
        };
        let script = elevation_script(&command, DEFAULT_TIMEOUT);
        let encoded = encode_powershell(&script);
        let utf16_bytes = script.encode_utf16().count() * 2;
        assert_eq!(encoded.len(), utf16_bytes.div_ceil(3) * 4);
        assert!(encoded
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '/' | '=')));
    }

    #[cfg(not(windows))]
    #[test]
    fn elevation_is_unsupported_off_windows() {
        assert!(!supported());
        let result = run(
            &request(
                ElevationOperation::GrantPathAcl,
                vec!["C:\\Program Files\\Sunshine"],
                "reason",
            ),
            &ElevationRoots::default(),
        );
        assert_eq!(result.status, ElevationStatus::Unsupported);
        assert_eq!(
            result.message,
            "Elevation is only available on Windows; nothing was changed."
        );
        assert_eq!(result.exit_code, None);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "unsupported");
    }
}
