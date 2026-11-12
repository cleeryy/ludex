//! Platform and provider detection for "Make it run" (#178).
//!
//! Everything observable goes through [`Probe`]: the real implementation reads
//! the filesystem and, on Windows, `reg.exe` with a fixed argument list. Tests
//! inject [`FixtureProbe`], so the catalog, plan and wizard tests never depend
//! on the machine running them.

use serde::Serialize;
#[cfg(test)]
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{Platform, ProviderKind, RuntimeCatalog, RuntimeInstallRow, RuntimeProvider};

/// CPU architecture in the vendor-neutral spelling used by the catalog.
pub fn normalize_arch(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "amd64" | "x86_64" | "x64" => "x86_64".to_string(),
        "aarch64" | "arm64" => "aarch64".to_string(),
        "x86" | "i386" | "i686" => "x86".to_string(),
        other => other.to_string(),
    }
}

/// Facts about the machine that do not come from the catalog.
#[derive(Debug, Clone)]
pub struct HostInfo {
    pub platform: Platform,
    pub arch: String,
    pub os_version: Option<String>,
    pub elevated: bool,
    pub home: Option<PathBuf>,
    pub app_data: PathBuf,
    pub cache_dir: PathBuf,
    pub system_root: PathBuf,
    pub free_bytes: Option<u64>,
}

impl HostInfo {
    /// Read the host once. `app_data` is the Tauri app-data folder.
    pub fn current(app_data: PathBuf) -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        let system_root = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        let cache_dir = app_data.join("runtime-cache");
        let free_bytes = fs2::available_space(
            crate::paths::nearest_existing(&cache_dir).unwrap_or_else(|| app_data.clone()),
        )
        .ok();
        Self {
            platform: Platform::current(),
            arch: normalize_arch(std::env::consts::ARCH),
            os_version: os_version(),
            elevated: is_elevated(),
            home,
            app_data,
            cache_dir,
            system_root,
            free_bytes,
        }
    }

    pub fn is_macos_15_or_newer(&self) -> Option<bool> {
        let version = self.os_version.as_deref()?;
        Some(version_at_least(version, "15.0"))
    }
}

fn os_version() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!value.is_empty()).then_some(value)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// True when the current process can install machine-wide prerequisites.
fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        // A fixed call, no shell. High integrity (S-1-16-12288) or System
        // (S-1-16-16384) means the UAC prompt was already accepted.
        match Command::new("whoami").args(["/groups"]).output() {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                text.contains("S-1-16-12288") || text.contains("S-1-16-16384")
            }
            _ => false,
        }
    }
    #[cfg(unix)]
    {
        match Command::new("id").arg("-u").output() {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim() == "0"
            }
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// Everything detection may ask about the machine.
pub trait Probe {
    fn path_exists(&self, path: &Path) -> bool;
    fn dir_exists(&self, path: &Path) -> bool;
    fn file_size(&self, path: &Path) -> Option<u64>;
    fn list_dir(&self, path: &Path) -> Vec<String>;
    fn registry_value(&self, key: &str, value: &str) -> Option<String>;
    fn registry_key_exists(&self, key: &str) -> bool;
}

/// Reads the real filesystem and registry. Never writes anything.
pub struct RealProbe;

impl Probe for RealProbe {
    fn path_exists(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn dir_exists(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn file_size(&self, path: &Path) -> Option<u64> {
        std::fs::metadata(path).ok().map(|metadata| metadata.len())
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        names.sort();
        names
    }

    fn registry_value(&self, key: &str, value: &str) -> Option<String> {
        #[cfg(windows)]
        {
            let output = Command::new("reg")
                .args(["query", key, "/v", value])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                let mut fields = line.split_whitespace();
                if fields.next() == Some(value) {
                    // REG_SZ / REG_DWORD / REG_EXPAND_SZ, then the data.
                    let _kind = fields.next();
                    let data = fields.collect::<Vec<_>>().join(" ");
                    if !data.is_empty() {
                        return Some(data);
                    }
                }
            }
            None
        }
        #[cfg(not(windows))]
        {
            let _ = (key, value);
            None
        }
    }

    fn registry_key_exists(&self, key: &str) -> bool {
        #[cfg(windows)]
        {
            Command::new("reg")
                .args(["query", key])
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(windows))]
        {
            let _ = key;
            false
        }
    }
}

/// Deterministic probe used by the unit tests.
#[cfg(test)]
#[derive(Default)]
pub struct FixtureProbe {
    pub files: std::collections::BTreeMap<PathBuf, u64>,
    pub dirs: BTreeSet<PathBuf>,
    pub registry: std::collections::BTreeMap<(String, String), String>,
    pub registry_keys: BTreeSet<String>,
}

#[cfg(test)]
impl FixtureProbe {
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.insert(path.into(), 0);
        self
    }

    pub fn file_with_size(mut self, path: impl Into<PathBuf>, size: u64) -> Self {
        self.files.insert(path.into(), size);
        self
    }

    pub fn dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.dirs.insert(path.into());
        self
    }

    pub fn registry(mut self, key: &str, value: &str, data: &str) -> Self {
        self.registry
            .insert((key.to_string(), value.to_string()), data.to_string());
        self.registry_keys.insert(key.to_string());
        self
    }

    pub fn registry_key(mut self, key: &str) -> Self {
        self.registry_keys.insert(key.to_string());
        self
    }
}

#[cfg(test)]
impl Probe for FixtureProbe {
    fn path_exists(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn dir_exists(&self, path: &Path) -> bool {
        self.dirs.contains(path)
    }

    fn file_size(&self, path: &Path) -> Option<u64> {
        self.files.get(path).copied()
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let prefix = path.to_string_lossy().to_string();
        let mut names: Vec<String> = self
            .files
            .keys()
            .chain(self.dirs.iter())
            .filter_map(|candidate| {
                let text = candidate.to_string_lossy().to_string();
                let rest = text.strip_prefix(&format!("{prefix}/"))?;
                let name = rest.split('/').next()?;
                (!name.is_empty()).then(|| name.to_string())
            })
            .collect();
        names.sort();
        names.dedup();
        names
    }

    fn registry_value(&self, key: &str, value: &str) -> Option<String> {
        self.registry
            .get(&(key.to_string(), value.to_string()))
            .cloned()
    }

    fn registry_key_exists(&self, key: &str) -> bool {
        self.registry_keys.contains(key)
    }
}

/// A provider's state on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDetection {
    pub provider_id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub version: Option<String>,
    pub installed: bool,
    /// False for a provider that does not target this CPU architecture.
    pub applicable: bool,
    pub path: Option<String>,
    /// Human evidence: registry key, detected file, managed folder or import.
    pub evidence: String,
    pub managed: bool,
}

/// Optional GPU capability. Never blocks: a missing Vulkan ICD is a driver
/// problem Ludex does not install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuSupport {
    pub vulkan: Option<bool>,
    pub note: String,
}

pub fn detect_all(
    catalog: &RuntimeCatalog,
    host: &HostInfo,
    probe: &dyn Probe,
    records: &[RuntimeInstallRow],
) -> Vec<RuntimeDetection> {
    catalog
        .providers
        .iter()
        .filter(|provider| provider.platform == host.platform)
        .map(|provider| detect_provider(provider, host, probe, records))
        .collect()
}

pub fn detect_provider(
    provider: &RuntimeProvider,
    host: &HostInfo,
    probe: &dyn Probe,
    records: &[RuntimeInstallRow],
) -> RuntimeDetection {
    let applicable = provider
        .arch
        .as_deref()
        .map(|arch| normalize_arch(arch) == host.arch)
        .unwrap_or(true);
    let record = records.iter().find(|record| {
        record.provider_id == provider.id && record.platform == host.platform.as_str()
    });

    let mut installed = false;
    let mut evidence: Vec<String> = Vec::new();
    let mut version: Option<String> = None;
    let mut path: Option<String> = None;

    // 1. Registry checks. All of them must pass.
    if !provider.detection.registry.is_empty() {
        let mut all_passed = true;
        let mut seen_version: Option<String> = None;
        for check in &provider.detection.registry {
            let found = if check.value.is_empty() {
                probe.registry_key_exists(&check.key).then(String::new)
            } else {
                probe.registry_value(&check.key, &check.value)
            };
            let Some(value) = found else {
                all_passed = false;
                break;
            };
            if let Some(expected) = &check.expected {
                if !registry_matches(&value, expected) {
                    all_passed = false;
                    break;
                }
            }
            if let Some(minimum) = &check.min_version {
                if !version_at_least(&value, minimum) {
                    all_passed = false;
                    break;
                }
            }
            if seen_version.is_none() && looks_like_version(&value) {
                seen_version = Some(value.trim().to_string());
            }
            let suffix = if check.value.is_empty() {
                String::new()
            } else {
                format!("\\{}", check.value)
            };
            evidence.push(format!("registry {}{suffix}", check.key));
        }
        if all_passed {
            installed = true;
            version = seen_version;
        }
    }

    // 2. Files, folders and macOS app bundles.
    if !installed {
        for template in &provider.detection.paths {
            let candidate = expand(template, host);
            if probe.path_exists(&candidate) {
                installed = true;
                path = Some(candidate.to_string_lossy().to_string());
                evidence.push(format!("found {}", candidate.display()));
                break;
            }
        }
    }
    if !installed {
        for template in provider
            .detection
            .dirs
            .iter()
            .chain(provider.detection.app_bundles.iter())
        {
            let candidate = expand(template, host);
            if probe.dir_exists(&candidate) {
                installed = true;
                path = Some(candidate.to_string_lossy().to_string());
                evidence.push(format!("found {}", candidate.display()));
                break;
            }
        }
    }

    // 3. A runtime Ludex installed itself.
    if !installed {
        if let Some(record) = record {
            let record_path = PathBuf::from(&record.path);
            let imported = provider
                .install
                .as_ref()
                .map(|install| install.kind == super::InstallKind::Import)
                .unwrap_or(false);
            let entry_ok = if imported {
                probe.path_exists(&record_path)
                    && (record.size_bytes == 0
                        || probe.file_size(&record_path) == Some(record.size_bytes))
            } else if let Some(entry) = provider
                .download
                .as_ref()
                .and_then(|download| download.entry_point.as_deref())
            {
                probe.path_exists(&record_path.join(entry))
            } else {
                probe.dir_exists(&record_path) || probe.path_exists(&record_path)
            };
            if entry_ok {
                installed = true;
                path = Some(record.path.clone());
                version = Some(record.version.clone());
                evidence.push(if imported {
                    "imported by Ludex (evaluation licence)".to_string()
                } else {
                    "installed by Ludex".to_string()
                });
            } else if !record.path.is_empty() {
                evidence.push("the Ludex runtime record is stale".to_string());
            }
        }
    }

    // 4. Providers with no reliable detection key (DirectX): the audit record
    // is the only evidence Ludex has, and it is labelled as such.
    if !installed
        && provider.detection.registry.is_empty()
        && provider.detection.paths.is_empty()
        && provider.detection.dirs.is_empty()
        && provider.detection.app_bundles.is_empty()
        && provider
            .install
            .as_ref()
            .map(|install| install.kind != super::InstallKind::Import)
            .unwrap_or(true)
    {
        if let Some(record) = record {
            installed = true;
            path = Some(record.path.clone());
            version = Some(record.version.clone());
            evidence.push("recorded by Ludex (no reliable system key)".to_string());
        }
    }

    RuntimeDetection {
        provider_id: provider.id.clone(),
        name: provider.name.clone(),
        kind: provider.kind,
        version: version.or_else(|| installed.then(|| provider.version.clone())),
        installed,
        applicable,
        path,
        evidence: evidence.join("; "),
        managed: provider.managed,
    }
}

pub fn gpu_support(host: &HostInfo, probe: &dyn Probe) -> GpuSupport {
    match host.platform {
        Platform::Linux => {
            let roots = [
                PathBuf::from("/usr/share/vulkan/icd.d"),
                PathBuf::from("/usr/local/share/vulkan/icd.d"),
                host.home
                    .as_ref()
                    .map(|home| home.join(".local/share/vulkan/icd.d"))
                    .unwrap_or_else(|| PathBuf::from("/nonexistent")),
            ];
            let found = roots.iter().any(|root| {
                probe
                    .list_dir(root)
                    .iter()
                    .any(|name| name.ends_with(".json"))
            });
            GpuSupport {
                vulkan: Some(found),
                note: if found {
                    "A Vulkan ICD was found; DXVK can use it.".to_string()
                } else {
                    "No Vulkan ICD was found. Install your GPU driver or a Vulkan ICD; Ludex does not install drivers."
                        .to_string()
                },
            }
        }
        Platform::Windows => {
            let path = host.system_root.join("System32/vulkan-1.dll");
            let found = probe.path_exists(&path);
            GpuSupport {
                vulkan: Some(found),
                note: if found {
                    "The Vulkan loader is present.".to_string()
                } else {
                    "vulkan-1.dll was not found. Install the GPU driver; Ludex does not install drivers."
                        .to_string()
                },
            }
        }
        Platform::Macos => GpuSupport {
            vulkan: None,
            note: "macOS uses Metal; D3DMetal/GPTK or the wrapper provides translation."
                .to_string(),
        },
    }
}

fn expand(template: &str, host: &HostInfo) -> PathBuf {
    let mut value = template.to_string();
    if let Some(home) = &host.home {
        value = value.replace("{home}", &home.to_string_lossy());
    }
    value = value.replace("{appData}", &host.app_data.to_string_lossy());
    value = value.replace("{systemRoot}", &host.system_root.to_string_lossy());
    PathBuf::from(value)
}

/// Compare a registry value. `reg.exe` prints `REG_DWORD` as `0x1`, while the
/// catalog writes the logical value (`1`).
pub fn registry_matches(value: &str, expected: &str) -> bool {
    let value = value.trim();
    if value.eq_ignore_ascii_case(expected) {
        return true;
    }
    value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .map(|number| number.to_string() == expected)
        .unwrap_or(false)
}

/// Dotted numeric comparison (`14.44.35211` >= `14.0`). Non-numeric parts stop
/// the comparison, so a malformed registry value fails closed.
pub fn version_at_least(value: &str, minimum: &str) -> bool {
    let parse = |text: &str| -> Vec<u64> {
        text.trim()
            .split('.')
            .map(|part| {
                part.chars()
                    .take_while(|character| character.is_ascii_digit())
                    .collect::<String>()
                    .parse::<u64>()
                    .unwrap_or(0)
            })
            .collect()
    };
    let mut left = parse(value);
    let mut right = parse(minimum);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let length = left.len().max(right.len());
    left.resize(length, 0);
    right.resize(length, 0);
    left >= right
}

fn looks_like_version(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value
            .chars()
            .next()
            .map(|character| character.is_ascii_digit())
            .unwrap_or(false)
        && value.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::catalog;

    fn host(platform: Platform, arch: &str) -> HostInfo {
        HostInfo {
            platform,
            arch: arch.to_string(),
            os_version: Some("15.3".to_string()),
            elevated: false,
            home: Some(PathBuf::from("/home/tester")),
            app_data: PathBuf::from("/home/tester/.local/share/ludex"),
            cache_dir: PathBuf::from("/home/tester/.local/share/ludex/runtime-cache"),
            system_root: PathBuf::from("C:\\Windows"),
            free_bytes: Some(50 * 1024 * 1024 * 1024),
        }
    }

    fn record(provider_id: &str, platform: Platform, path: &str) -> RuntimeInstallRow {
        RuntimeInstallRow {
            provider_id: provider_id.to_string(),
            platform: platform.as_str().to_string(),
            version: "1.4.4".to_string(),
            path: path.to_string(),
            sha256: String::new(),
            size_bytes: 0,
            installed_at: "2026-11-12T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn managed_umu_is_detected_from_its_recorded_folder() {
        let catalog = catalog::build();
        let provider = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "umu-launcher")
            .unwrap();
        let root = PathBuf::from("/home/tester/.local/share/ludex/runtimes/umu-launcher/1.4.4");
        let probe = FixtureProbe::default().file(root.join("umu/umu-run"));
        let detection = detect_provider(
            provider,
            &host(Platform::Linux, "x86_64"),
            &probe,
            &[record(
                "umu-launcher",
                Platform::Linux,
                &root.to_string_lossy(),
            )],
        );
        assert!(detection.installed);
        assert_eq!(detection.version.as_deref(), Some("1.4.4"));
        assert!(detection.evidence.contains("installed by Ludex"));
    }

    #[test]
    fn a_stale_managed_record_is_not_reported_installed() {
        let catalog = catalog::build();
        let provider = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "umu-launcher")
            .unwrap();
        let probe = FixtureProbe::default();
        let detection = detect_provider(
            provider,
            &host(Platform::Linux, "x86_64"),
            &probe,
            &[record(
                "umu-launcher",
                Platform::Linux,
                "/home/tester/.local/share/ludex/runtimes/umu-launcher/1.4.4",
            )],
        );
        assert!(!detection.installed);
        assert!(detection.evidence.contains("stale"));
    }

    #[test]
    fn registry_requires_installed_flag_and_minimum_version() {
        let catalog = catalog::build();
        let provider = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "vcrun-2022-x64")
            .unwrap();
        let key = "HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64";

        let good = FixtureProbe::default()
            .registry(key, "Installed", "1")
            .registry(key, "Version", "14.44.35211");
        let detection = detect_provider(provider, &host(Platform::Windows, "x86_64"), &good, &[]);
        assert!(detection.installed);
        assert_eq!(detection.version.as_deref(), Some("14.44.35211"));
        assert!(detection.evidence.contains("registry"));

        let old = FixtureProbe::default()
            .registry(key, "Installed", "1")
            .registry(key, "Version", "13.9.1");
        let detection = detect_provider(provider, &host(Platform::Windows, "x86_64"), &old, &[]);
        assert!(
            !detection.installed,
            "an older runtime does not satisfy 14.x"
        );

        let off = FixtureProbe::default()
            .registry(key, "Installed", "0")
            .registry(key, "Version", "14.44.35211");
        assert!(
            !detect_provider(provider, &host(Platform::Windows, "x86_64"), &off, &[]).installed
        );
    }

    #[test]
    fn dotnet_is_detected_by_its_key_and_directx_by_its_record() {
        let catalog = catalog::build();
        let dotnet = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "dotnet-desktop-10")
            .unwrap();
        let key = "HKLM\\SOFTWARE\\dotnet\\Setup\\InstalledVersions\\x64\\sharedfx\\Microsoft.WindowsDesktop.App";
        let probe = FixtureProbe::default().registry_key(key);
        assert!(detect_provider(dotnet, &host(Platform::Windows, "x86_64"), &probe, &[]).installed);

        let directx = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "directx-jun2010")
            .unwrap();
        let probe = FixtureProbe::default();
        let detection = detect_provider(
            directx,
            &host(Platform::Windows, "x86_64"),
            &probe,
            &[record(
                "directx-jun2010",
                Platform::Windows,
                "cache/directx",
            )],
        );
        assert!(detection.installed);
        assert!(detection.evidence.contains("no reliable system key"));
    }

    #[test]
    fn macos_app_bundles_and_architecture_are_respected() {
        let catalog = catalog::build();
        let crossover = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "crossover")
            .unwrap();
        let probe = FixtureProbe::default().dir("/Applications/CrossOver.app");
        let detection = detect_provider(crossover, &host(Platform::Macos, "aarch64"), &probe, &[]);
        assert!(detection.installed);
        assert_eq!(
            detection.path.as_deref(),
            Some("/Applications/CrossOver.app")
        );

        let whisky = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "whisky")
            .unwrap();
        let intel = detect_provider(whisky, &host(Platform::Macos, "x86_64"), &probe, &[]);
        assert!(!intel.applicable, "Whisky is Apple Silicon only");
    }

    #[test]
    fn imported_gptk_is_installed_only_when_the_file_matches() {
        let catalog = catalog::build();
        let gptk = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "gptk")
            .unwrap();
        let mut row = record("gptk", Platform::Macos, "/Users/tester/Downloads/gptk.dmg");
        row.size_bytes = 1234;

        let matching =
            FixtureProbe::default().file_with_size("/Users/tester/Downloads/gptk.dmg", 1234);
        let detection = detect_provider(
            gptk,
            &host(Platform::Macos, "aarch64"),
            &matching,
            &[row.clone()],
        );
        assert!(detection.installed);
        assert!(detection.evidence.contains("evaluation licence"));

        let changed =
            FixtureProbe::default().file_with_size("/Users/tester/Downloads/gptk.dmg", 99);
        assert!(
            !detect_provider(gptk, &host(Platform::Macos, "aarch64"), &changed, &[row]).installed
        );
    }

    #[test]
    fn registry_dword_values_are_compared_numerically() {
        assert!(registry_matches("1", "1"));
        assert!(registry_matches("0x1", "1"));
        assert!(!registry_matches("0x0", "1"));
        assert!(registry_matches("14.44.35211", "14.44.35211"));
    }

    #[test]
    fn version_compare_handles_short_and_dotted_values() {
        assert!(version_at_least("14.44.35211", "14.0"));
        assert!(version_at_least("15", "15.0"));
        assert!(!version_at_least("14.0", "14.1"));
        assert!(!version_at_least("nonsense", "1.0"));
    }

    #[test]
    fn vulkan_detection_reads_icd_json_files() {
        let host = host(Platform::Linux, "x86_64");
        let empty = FixtureProbe::default();
        assert_eq!(gpu_support(&host, &empty).vulkan, Some(false));

        let with_icd = FixtureProbe::default().file("/usr/share/vulkan/icd.d/nvidia_icd.json");
        assert_eq!(gpu_support(&host, &with_icd).vulkan, Some(true));
    }
}
