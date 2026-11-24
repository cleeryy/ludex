//! One-click launch diagnostics bundle (#298).
//!
//! A failed launch needs evidence: the bounded per-launch compatibility log
//! (#280), the plan that was resolved, the prefix layout, the GPU driver and
//! the UMU/Proton/Wine versions. This module collects exactly that, zips it and
//! redacts it before the file is offered for a report.
//!
//! Rules encoded here:
//!
//! * nothing the user owns is included — no game files, no saves, no account
//!   token; the bundle carries `manifest.json`, `report.md` and the
//!   `launch-*.log` files of the game;
//! * secrets are scrubbed twice: environment values whose key names a secret
//!   are masked before serialization, then `crate::bridge::redact` removes
//!   tokens, passwords, server URLs, e-mail addresses and absolute paths;
//! * the home directory is removed before the generic path scanner runs, so no
//!   absolute home path leaves the machine; remaining paths become
//!   `<path:hash>`, and the account name and hostname are replaced too;
//! * the plan is represented by its SHA-256 ([`crate::launch_recovery`]), never
//!   by the user's files.
//!
//! Collection is pure filesystem reads and `sysinfo` lookups, so tests build a
//! bundle from fixture directories without a real GPU or Steam install.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bridge::redact::{redact, redact_json, RedactionContext};
use crate::compat::CompatScan;
use crate::launch_recovery::{FailureRecord, PlanSnapshot};
use crate::linux_gaming::LaunchOptions;

/// Newest `launch-*.log` files kept per game, matching the retention in
/// `launch.rs` (#280).
pub const MAX_LOGS: usize = 5;
/// Largest slice read from one launch log (already trimmed to 512 KiB, #280).
pub const MAX_LOG_BYTES: u64 = 512 * 1024;
/// Largest report body put in the GitHub issue URL.
const MAX_REPORT_URL_CHARS: usize = 1_800;
/// Environment names that always carry a credential.
const SECRET_MARKERS: &[&str] = &["TOKEN", "PASS", "SECRET", "KEY", "CREDENTIAL", "AUTH"];

/// One GPU and the kernel driver bound to it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GpuInfo {
    /// `0x10de`, `0x1002`, `0x8086` or empty when unreadable.
    pub vendor_id: String,
    /// Friendly vendor from the PCI id.
    pub vendor: String,
    /// Kernel driver (`nvidia`, `amdgpu`, `i915`, `nouveau`…).
    pub driver: String,
    /// NVIDIA driver version from `/proc/driver/nvidia/version`, when present.
    pub driver_version: Option<String>,
}

/// One detected compatibility tool, for the versions row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolVersion {
    /// `umu`, `proton`, `wine` or `gamescope`.
    pub kind: String,
    pub name: String,
    pub version: String,
}

/// Host facts that explain a launch without naming the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostFacts {
    pub distro: String,
    pub distro_id: String,
    pub kernel: String,
    pub gpus: Vec<GpuInfo>,
}

/// What the game prefix looks like right now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrefixHealth {
    pub path: String,
    /// True when `prefixPath` came from the per-game options, not the default.
    pub custom: bool,
    pub exists: bool,
    /// `<prefix>/pfx` exists (the Proton layout).
    pub has_pfx: bool,
    pub has_system_reg: bool,
    pub system_reg_bytes: u64,
    pub has_drive_c: bool,
}

impl PrefixHealth {
    /// One sentence the report can quote.
    pub fn summary(&self) -> &'static str {
        if !self.exists {
            "the prefix does not exist yet"
        } else if !self.has_system_reg && !self.has_pfx {
            "the prefix exists but has no Wine system registry or pfx folder"
        } else if !self.has_drive_c {
            "the prefix has no drive_c"
        } else {
            "the prefix looks complete"
        }
    }
}

/// One launch log read for the bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFile {
    pub name: String,
    pub text: String,
}

/// Everything the bundle needs. Tests build it directly; the command fills it
/// from `AppState`.
pub struct BundleInput {
    pub game_id: i64,
    pub title: String,
    /// RFC 3339 UTC.
    pub created_at: String,
    pub app_version: String,
    pub host: HostFacts,
    pub tools: Vec<ToolVersion>,
    pub options: LaunchOptions,
    pub plan: Option<PlanSnapshot>,
    pub prefix: Option<PrefixHealth>,
    pub last_failure: Option<FailureRecord>,
    pub logs: Vec<LogFile>,
    pub redaction: RedactionContext,
}

/// The written bundle and the metadata a "report this game" click uses.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub path: PathBuf,
    pub file_name: String,
    pub bytes: u64,
    /// Hash of the plan that ran (or of the current options when none ran).
    pub plan_hash: String,
    /// GitHub new-issue URL prefilled with the report metadata.
    pub report_url: String,
}

/// Collect distro, kernel and GPU facts from the running host.
pub fn collect_host() -> HostFacts {
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let (distro, distro_id) = parse_os_release(&os_release);
    let kernel = sysinfo::System::kernel_version()
        .or_else(|_| sysinfo::System::os_version())
        .unwrap_or_default();
    HostFacts {
        distro,
        distro_id,
        kernel,
        gpus: collect_gpus(),
    }
}

/// Parse `PRETTY_NAME` and `ID` from an os-release file.
pub fn parse_os_release(text: &str) -> (String, String) {
    let mut pretty = String::new();
    let mut id = String::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key {
            "PRETTY_NAME" if pretty.is_empty() => pretty = value,
            "NAME" if pretty.is_empty() => pretty = value,
            "ID" => id = value,
            _ => {}
        }
    }
    if pretty.is_empty() {
        pretty = sysinfo::System::long_os_version()
            .or_else(|_| sysinfo::System::os_version())
            .unwrap_or_else(|_| std::env::consts::OS.to_string());
    }
    (pretty, id)
}

/// Read `/sys/class/drm/card*` and the NVIDIA version, without shelling out.
pub fn collect_gpus() -> Vec<GpuInfo> {
    let nvidia_version = std::fs::read_to_string("/proc/driver/nvidia/version")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                line.split_whitespace()
                    .find(|part| {
                        part.chars().next().is_some_and(|c| c.is_ascii_digit())
                            && part.contains('.')
                    })
                    .map(str::to_string)
            })
        });
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut cards: Vec<(String, GpuInfo)> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            // `card0`, `card1`; `card0-DP-1` are connectors, not the card.
            let index = name.strip_prefix("card")?.to_string();
            if !index.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let device = entry.path().join("device");
            let vendor_id = std::fs::read_to_string(device.join("vendor"))
                .map(|value| value.trim().to_string())
                .unwrap_or_default();
            let driver = std::fs::read_link(device.join("driver"))
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                })
                .unwrap_or_default();
            let vendor = match vendor_id.as_str() {
                "0x10de" => "NVIDIA",
                "0x1002" => "AMD",
                "0x8086" => "Intel",
                _ => "unknown",
            }
            .to_string();
            let version = driver
                .eq_ignore_ascii_case("nvidia")
                .then(|| nvidia_version.clone())
                .flatten();
            Some((
                index,
                GpuInfo {
                    vendor_id,
                    vendor,
                    driver,
                    driver_version: version,
                },
            ))
        })
        .collect();
    cards.sort_by(|left, right| left.0.cmp(&right.0));
    cards.into_iter().map(|(_, card)| card).collect()
}

/// The versions row: UMU, Proton, Wine and Gamescope as the scan saw them.
pub fn collect_tools(scan: Option<&CompatScan>) -> Vec<ToolVersion> {
    let Some(scan) = scan else {
        return Vec::new();
    };
    let mut tools: Vec<ToolVersion> = scan
        .tools
        .iter()
        .map(|tool| {
            // UMU does not ship a `version` file; ask the launcher itself. A
            // failing probe only leaves the version empty, never fails the
            // bundle.
            let version = tool
                .version
                .clone()
                .or_else(|| {
                    (tool.kind == crate::compat::ToolKind::Umu)
                        .then(|| probe_version(&tool.path))
                        .flatten()
                })
                .unwrap_or_default();
            ToolVersion {
                kind: tool.kind.as_str().to_string(),
                name: tool.name.clone(),
                version,
            }
        })
        .collect();
    if let Some(gamescope) = &scan.gamescope {
        tools.push(ToolVersion {
            kind: "gamescope".to_string(),
            name: "gamescope".to_string(),
            version: gamescope.version.clone().unwrap_or_default(),
        });
    }
    tools.truncate(12);
    tools
}

/// Best-effort `--version` for a launcher that ships no version file (UMU).
fn probe_version(program: &str) -> Option<String> {
    let output = crate::process::capture(program)
        .arg("--version")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Look at the prefix layout; never walks the whole tree.
pub fn prefix_health(prefix: &Path, custom: bool) -> PrefixHealth {
    let exists = prefix.is_dir();
    let system_reg = prefix.join("system.reg");
    let metadata = std::fs::metadata(&system_reg).ok();
    PrefixHealth {
        path: prefix.to_string_lossy().to_string(),
        custom,
        exists,
        has_pfx: prefix.join("pfx").is_dir(),
        has_system_reg: metadata.is_some(),
        system_reg_bytes: metadata.map(|meta| meta.len()).unwrap_or(0),
        has_drive_c: prefix.join("drive_c").is_dir() || prefix.join("pfx/drive_c").is_dir(),
    }
}

/// The game's retained `launch-*.log` files, newest first (#280).
pub fn read_launch_logs(app_data: &Path, game_id: i64) -> Vec<LogFile> {
    let dir = app_data.join(crate::logging::LOG_SUBDIR);
    let prefix = format!("launch-{game_id}-");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            name.starts_with(&prefix) && name.ends_with(".log")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    logs.truncate(MAX_LOGS);
    logs.into_iter()
        .filter_map(|(_, path)| {
            let name = path.file_name()?.to_string_lossy().to_string();
            let file = std::fs::File::open(&path).ok()?;
            let mut bytes = Vec::new();
            file.take(MAX_LOG_BYTES).read_to_end(&mut bytes).ok()?;
            Some(LogFile {
                name,
                text: String::from_utf8_lossy(&bytes).to_string(),
            })
        })
        .collect()
}

/// Mask environment values whose key names a credential.
fn mask_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(key, value)| {
            let upper = key.to_ascii_uppercase();
            let secret = SECRET_MARKERS.iter().any(|marker| upper.contains(marker));
            (
                key.clone(),
                if secret {
                    "<redacted>".to_string()
                } else {
                    value.clone()
                },
            )
        })
        .collect()
}

fn masked_options(options: &LaunchOptions) -> Value {
    json!({
        "mode": options.mode.as_str(),
        "toolId": options.tool_id,
        "prefixPath": options.prefix_path,
        "env": mask_env(&options.env),
        "winetricksVerbs": options.winetricks_verbs,
        "steamAppId": options.steam_app_id,
        "workingDir": options.working_dir,
        "display": {
            "mode": if options.display.is_gamescope() { "gamescope" } else { "desktop" },
            "width": options.display.width,
            "height": options.display.height,
            "refresh": options.display.refresh,
            "filter": options.display.filter,
            "adaptiveSync": options.display.adaptive_sync,
            "hdr": options.display.hdr,
        },
    })
}

fn masked_resolved(plan: &PlanSnapshot) -> Value {
    match &plan.resolved {
        Some(resolved) => json!({
            "program": resolved.program,
            "args": resolved.args,
            "env": mask_env(&resolved.env),
            "prefix": resolved.prefix,
            "toolId": resolved.tool_id,
            "toolName": resolved.tool_name,
            "display": resolved.display,
        }),
        None => Value::Null,
    }
}

/// Build the manifest object *before* redaction, with secrets already masked.
fn manifest(input: &BundleInput) -> Value {
    json!({
        "schema": 1,
        "kind": "ludex-launch-bundle",
        "createdAt": input.created_at,
        "app": {
            "version": input.app_version,
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "host": {
            "distro": input.host.distro,
            "distroId": input.host.distro_id,
            "kernel": input.host.kernel,
            "gpus": input.host.gpus.iter().map(|gpu| json!({
                "vendorId": gpu.vendor_id,
                "vendor": gpu.vendor,
                "driver": gpu.driver,
                "driverVersion": gpu.driver_version,
            })).collect::<Vec<_>>(),
        },
        "tools": input.tools.iter().map(|tool| json!({
            "kind": tool.kind,
            "name": tool.name,
            "version": tool.version,
        })).collect::<Vec<_>>(),
        "game": {
            "id": input.game_id,
            "title": input.title,
        },
        "plan": {
            "hash": Value::Null, // inserted after redaction; the hash itself is safe
            "options": masked_options(&input.options),
            "resolved": input.plan.as_ref().map(masked_resolved).unwrap_or(Value::Null),
        },
        "prefix": input.prefix.as_ref().map(|prefix| json!({
            "custom": prefix.custom,
            "exists": prefix.exists,
            "hasPfx": prefix.has_pfx,
            "hasSystemReg": prefix.has_system_reg,
            "systemRegBytes": prefix.system_reg_bytes,
            "hasDriveC": prefix.has_drive_c,
            "health": prefix.summary(),
        })).unwrap_or(Value::Null),
        "failures": input.last_failure.as_ref().map(|failure| json!({
            "at": failure.at,
            "exitCode": failure.exit_code,
            "seconds": failure.seconds,
        })).unwrap_or(Value::Null),
        "contents": {
            "logs": input.logs.iter().map(|log| log.name.clone()).collect::<Vec<_>>(),
            "note": "No game files, saves, account tokens or passwords are included.",
        },
    })
}

fn hex_exit(code: Option<u32>) -> String {
    match code {
        Some(code) => format!("{code} (0x{code:08X})"),
        None => "none reported (signal)".to_string(),
    }
}

/// The Markdown report. Redacted before it is written and linked.
fn report_markdown(input: &BundleInput, plan_hash: &str, manifest: &Value) -> String {
    let host = &input.host;
    let gpus = if host.gpus.is_empty() {
        "not detected".to_string()
    } else {
        host.gpus
            .iter()
            .map(|gpu| {
                let version = gpu
                    .driver_version
                    .as_deref()
                    .map(|version| format!(" {version}"))
                    .unwrap_or_default();
                format!("{} {}{}", gpu.vendor, gpu.driver, version)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let tools = if input.tools.is_empty() {
        "not detected".to_string()
    } else {
        input
            .tools
            .iter()
            .map(|tool| {
                if tool.version.is_empty() {
                    format!("{} {}", tool.kind, tool.name)
                } else {
                    format!("{} {} {}", tool.kind, tool.name, tool.version)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let prefix = input
        .prefix
        .as_ref()
        .map(|prefix| format!("{} ({})", prefix.summary(), prefix.path))
        .unwrap_or_else(|| "not recorded".to_string());
    let failure = match &input.last_failure {
        Some(failure) => format!(
            "last launch failed after {} s, exit {}",
            failure.seconds,
            hex_exit(failure.exit_code)
        ),
        None => "no failed launch recorded".to_string(),
    };
    let title = manifest
        .pointer("/game/title")
        .and_then(Value::as_str)
        .unwrap_or(&input.title);
    format!(
        "# Launch report: {title}\n\
         \n\
         - Ludex {version} · {platform} {arch}\n\
         - Plan hash: {plan_hash}\n\
         - Distribution: {distro}\n\
         - Kernel: {kernel}\n\
         - GPU: {gpus}\n\
         - Compatibility tools: {tools}\n\
         - Launch mode: {mode}\n\
         - Prefix: {prefix}\n\
         - {failure}\n\
         \n\
         Attach the zip produced by **Diagnostics bundle**. It contains the\n\
         resolved plan, the prefix health, the versions above and the retained\n\
         `launch-*.log` files, with tokens, passwords and the home path\n\
         removed. It contains no game files and no saves.\n",
        version = input.app_version,
        plan_hash = plan_hash,
        platform = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        distro = host.distro,
        kernel = host.kernel,
        mode = input.options.mode.as_str(),
    )
}

/// Percent-encode a query component (RFC 3986 unreserved set).
fn encode_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~');
        if keep {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn report_url(report: &str) -> String {
    let mut body: String = report.chars().take(MAX_REPORT_URL_CHARS).collect();
    if report.chars().count() > MAX_REPORT_URL_CHARS {
        body.push('…');
    }
    let title = report
        .lines()
        .next()
        .unwrap_or("Launch report")
        .trim_start_matches("# ")
        .to_string();
    format!(
        "https://github.com/cleeryy/ludex/issues/new?title={}&body={}",
        encode_component(&title),
        encode_component(&body)
    )
}

/// Redact a whole launch log without the per-field cap of `redact`.
///
/// `redact` bounds its output to 8,000 characters, which is right for a crash
/// report field and wrong for a 512 KiB launch log. Redacting line by line (and
/// splitting a pathological long line into chunks) keeps the whole log while
/// still running every value through the same scanners.
fn redact_log(text: &str, context: &RedactionContext) -> String {
    let limit = crate::bridge::redact::MAX_REDACTED_CHARS.saturating_sub(1_000);
    let mut output = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if line.chars().count() <= limit {
            output.push_str(&redact(line, context));
            continue;
        }
        let mut chunk = String::new();
        for character in line.chars() {
            chunk.push(character);
            if chunk.chars().count() >= limit {
                output.push_str(&redact(&chunk, context));
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            output.push_str(&redact(&chunk, context));
        }
    }
    output
}

/// Build and write the zip.
pub fn build(input: &BundleInput, output_dir: &Path) -> Result<Bundle, String> {
    std::fs::create_dir_all(output_dir)
        .map_err(|error| format!("Could not create {}: {error}", output_dir.display()))?;

    let plan_hash = input
        .plan
        .as_ref()
        .map(|plan| plan.hash.clone())
        .unwrap_or_else(|| crate::launch_recovery::plan_hash(&input.options, None, None));

    let manifest_value = redact_json(&manifest(input), &input.redaction);
    let mut manifest_value = manifest_value;
    if let Some(plan) = manifest_value
        .get_mut("plan")
        .and_then(Value::as_object_mut)
    {
        // The redactor treats a 64-hex string as a token; the plan hash is the
        // one opaque value a report must keep.
        plan.insert("hash".to_string(), Value::String(plan_hash.clone()));
    }
    let manifest_text = serde_json::to_string_pretty(&manifest_value)
        .map_err(|error| format!("Could not encode the bundle manifest: {error}"))?;

    const PLAN_HASH_MARKER: &str = "plan-hash-marker";
    let report = report_markdown(input, PLAN_HASH_MARKER, &manifest_value);
    // The redactor treats a 64-hex string as an opaque token; the plan hash is
    // the one opaque value a report must keep, so it is inserted after it.
    let report = redact(&report, &input.redaction).replace(PLAN_HASH_MARKER, &plan_hash);
    let logs: Vec<(String, String)> = input
        .logs
        .iter()
        .map(|log| (log.name.clone(), redact_log(&log.text, &input.redaction)))
        .collect();

    let file_name = format!(
        "launch-bundle-{}-{}.zip",
        input.game_id,
        file_stamp(&input.created_at)
    );
    let path = output_dir.join(&file_name);
    let file = std::fs::File::create(&path)
        .map_err(|error| format!("Could not create {}: {error}", path.display()))?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let write_entry = |writer: &mut zip::ZipWriter<std::fs::File>, name: &str, text: &str| {
        writer
            .start_file(name, options)
            .and_then(|_| {
                writer
                    .write_all(text.as_bytes())
                    .map_err(std::io::Error::other)
                    .map_err(zip::result::ZipError::Io)
            })
            .map_err(|error| format!("Could not write {name}: {error}"))
    };

    write_entry(&mut writer, "manifest.json", &manifest_text)?;
    write_entry(&mut writer, "report.md", &report)?;
    for (name, text) in &logs {
        write_entry(&mut writer, &format!("logs/{name}"), text)?;
    }
    writer
        .finish()
        .map_err(|error| format!("Could not finish the bundle: {error}"))?;

    let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    Ok(Bundle {
        path,
        file_name,
        bytes,
        plan_hash: plan_hash.clone(),
        report_url: report_url(&report),
    })
}

/// `2026-11-19T12:34:56Z` → `20261119-123456`.
fn file_stamp(created_at: &str) -> String {
    let digits: String = created_at
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect();
    if digits.len() >= 14 {
        format!("{}-{}", &digits[0..8], &digits[8..14])
    } else {
        "unknown".to_string()
    }
}

/// The command's return value.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchBundleInfo {
    pub path: String,
    pub file_name: String,
    pub bytes: u64,
    pub plan_hash: String,
    /// Prefilled GitHub issue URL: metadata only, no files.
    pub report_url: String,
}

impl From<Bundle> for LaunchBundleInfo {
    fn from(bundle: Bundle) -> Self {
        Self {
            path: bundle.path.to_string_lossy().to_string(),
            file_name: bundle.file_name,
            bytes: bundle.bytes,
            plan_hash: bundle.plan_hash,
            report_url: bundle.report_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_gaming::{DisplayMode, DisplayProfile, LaunchMode};
    use std::collections::BTreeMap;

    fn context() -> RedactionContext {
        RedactionContext::new()
            .with_home("/home/alice")
            .with_username("alice")
            .with_hostname("gaming-pc")
            .with_server_url("https://ludex.example")
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-bundle-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn options() -> LaunchOptions {
        let mut env = BTreeMap::new();
        env.insert("DXVK_ASYNC".to_string(), "1".to_string());
        env.insert(
            "STEAM_API_TOKEN".to_string(),
            "super-secret-token-value".to_string(),
        );
        LaunchOptions {
            mode: LaunchMode::Umu,
            tool_id: "umu:path".to_string(),
            prefix_path: "/home/alice/Games/prefix".to_string(),
            env,
            winetricks_verbs: Vec::new(),
            steam_app_id: Some(753640),
            working_dir: "/home/alice/Games/OuterWilds".to_string(),
            display: DisplayProfile {
                mode: DisplayMode::Desktop,
                ..DisplayProfile::default()
            },
            audio_sink: String::new(),
            gamemode: false,
        }
    }

    fn input(_dir: &Path) -> BundleInput {
        let mut plan = PlanSnapshot::new(
            &options(),
            Some("/home/alice/Games/OuterWilds/OuterWilds.exe"),
            None,
        );
        plan.resolved = Some(crate::launch_recovery::ResolvedPlan {
            program: "/usr/bin/umu-run".to_string(),
            args: vec!["/home/alice/Games/OuterWilds/OuterWilds.exe".to_string()],
            env: BTreeMap::from([(
                "PROTONPATH".to_string(),
                "/home/alice/.local/share/Steam/GE-Proton".to_string(),
            )]),
            prefix: "/home/alice/Games/prefix".to_string(),
            tool_id: "umu:path".to_string(),
            tool_name: "UMU".to_string(),
            display: "desktop".to_string(),
        });
        BundleInput {
            game_id: 42,
            title: "Outer Wilds".to_string(),
            created_at: "2026-11-19T12:34:56Z".to_string(),
            app_version: "0.4.25".to_string(),
            host: HostFacts {
                distro: "Arch Linux".to_string(),
                distro_id: "arch".to_string(),
                kernel: "6.12.9-arch1-1".to_string(),
                gpus: vec![GpuInfo {
                    vendor_id: "0x10de".to_string(),
                    vendor: "NVIDIA".to_string(),
                    driver: "nvidia".to_string(),
                    driver_version: Some("565.77".to_string()),
                }],
            },
            tools: vec![ToolVersion {
                kind: "umu".to_string(),
                name: "umu-run".to_string(),
                version: "1.1.4".to_string(),
            }],
            options: options(),
            plan: Some(plan),
            prefix: Some(PrefixHealth {
                path: "/home/alice/Games/prefix".to_string(),
                custom: false,
                exists: true,
                has_pfx: false,
                has_system_reg: true,
                system_reg_bytes: 2048,
                has_drive_c: true,
            }),
            last_failure: Some(FailureRecord {
                at: "2026-11-19T12:30:00Z".to_string(),
                exit_code: Some(53),
                seconds: 3,
            }),
            logs: vec![LogFile {
                name: "launch-42-abc.log".to_string(),
                text: "tool: UMU\npassword=hunter2\nAuthorization: Bearer abcdef123456\n\
                       prefix: /home/alice/Games/prefix\nuser alice on gaming-pc\n"
                    .to_string(),
            }],
            redaction: context(),
        }
    }

    fn zip_text(path: &Path, name: &str) -> String {
        let file = std::fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        text
    }

    #[test]
    fn bundle_contains_the_expected_entries() {
        let dir = temp_dir("entries");
        let input = input(&dir);
        let bundle = build(&input, &dir).unwrap();
        assert!(bundle.bytes > 0);
        assert_eq!(bundle.plan_hash, input.plan.as_ref().unwrap().hash);
        assert!(bundle.report_url.contains("issues/new"));
        assert!(bundle.report_url.contains(&bundle.plan_hash));

        let file = std::fs::File::open(&bundle.path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["logs/launch-42-abc.log", "manifest.json", "report.md",]
        );
        let manifest = zip_text(&bundle.path, "manifest.json");
        let value: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(value["kind"], "ludex-launch-bundle");
        assert_eq!(value["plan"]["hash"], bundle.plan_hash);
        assert_eq!(value["host"]["kernel"], "6.12.9-arch1-1");
        assert_eq!(value["tools"][0]["name"], "umu-run");
        let report = zip_text(&bundle.path, "report.md");
        assert!(
            report.contains(&bundle.plan_hash),
            "the report keeps the plan hash"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundle_redacts_secrets_and_home_paths() {
        let dir = temp_dir("redaction");
        let input = input(&dir);
        let bundle = build(&input, &dir).unwrap();
        let manifest = zip_text(&bundle.path, "manifest.json");
        let report = zip_text(&bundle.path, "report.md");
        let log = zip_text(&bundle.path, "logs/launch-42-abc.log");

        for text in [&manifest, &report, &log] {
            assert!(!text.contains("hunter2"), "password leaked: {text}");
            assert!(
                !text.contains("super-secret-token-value"),
                "token leaked: {text}"
            );
            assert!(!text.contains("abcdef123456"), "bearer leaked: {text}");
            assert!(!text.contains("/home/alice"), "home path leaked: {text}");
            assert!(!text.contains("alice"), "username leaked: {text}");
            assert!(!text.contains("gaming-pc"), "hostname leaked: {text}");
            assert!(!text.contains("ludex.example"), "server URL leaked: {text}");
        }
        assert!(
            log.contains("<path:") || log.contains('~'),
            "the prefix keeps a relative form"
        );
        assert!(
            manifest.contains("<redacted>"),
            "secret env keys are masked"
        );
        assert!(report.contains("Plan hash:"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn report_url_is_bounded_and_prefilled() {
        let dir = temp_dir("url");
        let mut input = input(&dir);
        input.logs.clear();
        let bundle = build(&input, &dir).unwrap();
        assert!(bundle.report_url.len() < 4_000);
        assert!(bundle.report_url.contains("Outer%20Wilds"));
        assert!(bundle.report_url.contains(&bundle.plan_hash));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prefix_health_reads_the_layout() {
        let root = temp_dir("prefix");
        let pfx = root.join("pfx");
        std::fs::create_dir_all(pfx.join("drive_c")).unwrap();
        std::fs::write(pfx.join("system.reg"), b"WINE REGISTRY").unwrap();
        let health = prefix_health(&root, true);
        assert!(health.exists && health.has_pfx && health.has_drive_c);
        assert!(
            !health.has_system_reg,
            "the Proton layout keeps it under pfx"
        );
        assert!(health.custom);
        assert_eq!(health.summary(), "the prefix looks complete");

        let missing = prefix_health(&root.join("nope"), false);
        assert_eq!(missing.summary(), "the prefix does not exist yet");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn os_release_is_parsed_without_quotes() {
        let (pretty, id) =
            parse_os_release("NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nID=arch\n");
        assert_eq!(pretty, "Arch Linux");
        assert_eq!(id, "arch");
    }

    #[test]
    fn launch_logs_are_limited_to_the_game() {
        let root = temp_dir("logs");
        let logs = root.join(crate::logging::LOG_SUBDIR);
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("launch-42-a.log"), b"one").unwrap();
        std::fs::write(logs.join("launch-42-b.log"), b"two").unwrap();
        std::fs::write(logs.join("launch-7-c.log"), b"other").unwrap();
        std::fs::write(logs.join("ludex.log"), b"app").unwrap();

        let files = read_launch_logs(&root, 42);
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|log| log.name.starts_with("launch-42-")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn long_logs_keep_their_tail_after_redaction() {
        let mut text = String::from("=== Ludex compatibility launch ===\n");
        for index in 0..600 {
            text.push_str(&format!(
                "line {index}: Proton: Error: unable to use parent for game drive\n"
            ));
        }
        text.push_str("FINAL_MARKER exit status: 53\n");
        assert!(text.len() > 20_000);
        let redacted = redact_log(&text, &context());
        assert!(redacted.contains("FINAL_MARKER"), "the tail is kept");
        assert!(redacted.len() > 15_000, "the log is not truncated to 8 K");
        assert!(!redacted.contains("/home/alice"));
    }

    #[test]
    fn file_stamp_is_sortable() {
        assert_eq!(file_stamp("2026-11-19T12:34:56Z"), "20261119-123456");
        assert_eq!(file_stamp("broken"), "unknown");
    }
}
