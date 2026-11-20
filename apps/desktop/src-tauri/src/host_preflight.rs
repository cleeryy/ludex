//! Host preflight (#285): what this machine is still missing to play games.
//!
//! The guided "Make it run" wizard (#178) asks the user to choose components.
//! This module answers the question that comes before it: on a clean machine,
//! which pieces are absent, which are present but wrong, and what exactly does
//! the distribution call the package that provides them?
//!
//! Rules encoded here:
//! - every probe returns `missing` / `tooOld` / `ok` / `unknown` **and** one
//!   human sentence, so the UI never shows a bare identifier;
//! - the package names are a data table ([`RECIPES`]) with one row per finding
//!   and one column per package family; a family without a package gets a
//!   `manual` instruction with an official link instead of an invented name;
//! - nothing is installed without [`HostInstallPlan`] listing the exact
//!   packages and an explicit `consent` flag;
//! - GPU drivers are never in the table. The GPU finding detects, explains and
//!   links; [`plan_install`] refuses it even when a caller asks by name;
//! - the last result is stored in [`LocalSettings::host_preflight`], so a
//!   launch does not re-probe. `host_preflight_report(refresh = true)` is the
//!   manual "Re-run checks" and replaces it.
//!
//! Probe inputs are explicit ([`ProbeEnv`]) so fixture directory trees can be
//! classified in tests without a real Linux host. Command probes (`lspci`,
//! `nvidia-smi`, `vulkaninfo`) are bounded by [`PROBE_TIMEOUT`]; installing is
//! bounded by [`INSTALL_TIMEOUT`]. All of it runs on a blocking thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::compat::{self, CompatScan, ToolKind};
use crate::settings::LocalSettings;
use crate::state::AppState;

/// A single probe command may not take longer than this.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
/// One install command (polkit prompt included) may not take longer than this.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// Output kept from an install command, in characters.
const MAX_OUTPUT_CHARS: usize = 8_000;

/* ------------------------------- findings -------------------------------- */

pub const FINDING_GPU: &str = "gpu";
pub const FINDING_VULKAN: &str = "vulkan";
pub const FINDING_VULKAN32: &str = "vulkan32";
pub const FINDING_UMU: &str = "umu";
pub const FINDING_PROTON: &str = "proton";
pub const FINDING_GAMEMODE: &str = "gamemode";
pub const FINDING_MANGOHUD: &str = "mangohud";
pub const FINDING_GAMESCOPE: &str = "gamescope";
pub const FINDING_WEBKIT: &str = "webkitgtk";

/// The order findings are drawn in.
pub const FINDING_ORDER: [&str; 9] = [
    FINDING_GPU,
    FINDING_VULKAN,
    FINDING_VULKAN32,
    FINDING_WEBKIT,
    FINDING_UMU,
    FINDING_PROTON,
    FINDING_GAMEMODE,
    FINDING_MANGOHUD,
    FINDING_GAMESCOPE,
];

/// How one probe ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum HostStatus {
    /// Not present: installing the recipe (or a manual step) fixes it.
    Missing,
    /// Present but not good enough; the sentence explains why.
    TooOld,
    /// Present and usable.
    Ok,
    /// The probe could not tell (no GPU visible, unsupported OS, …).
    #[default]
    Unknown,
}

/// Package manager family, from `/etc/os-release` and the binaries on `PATH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PackageFamily {
    Pacman,
    Dnf,
    Apt,
    Zypper,
    #[default]
    Unknown,
}

impl PackageFamily {
    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            PackageFamily::Pacman => "pacman",
            PackageFamily::Dnf => "dnf",
            PackageFamily::Apt => "apt",
            PackageFamily::Zypper => "zypper",
            PackageFamily::Unknown => "unknown",
        }
    }

    /// Omitted packages for this family; `None` when nothing can be installed.
    fn packages(self, recipe: &Recipe) -> Option<&'static [&'static str]> {
        let list = match self {
            PackageFamily::Pacman => recipe.pacman,
            PackageFamily::Dnf => recipe.dnf,
            PackageFamily::Apt => recipe.apt,
            PackageFamily::Zypper => recipe.zypper,
            PackageFamily::Unknown => return None,
        };
        if list.is_empty() {
            None
        } else {
            Some(list)
        }
    }
}

/// Detected distribution family, with the sentence the UI shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HostFamily {
    pub id: PackageFamily,
    /// Human label: `Arch Linux (pacman)`.
    pub label: String,
    /// How it was detected, e.g. `ID=cachyos in /etc/os-release`.
    pub detail: String,
    /// False for `unknown`: the UI hides "Install missing".
    pub installable: bool,
}

/// A manual instruction for a family without a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostManual {
    pub instruction: String,
    pub url: Option<String>,
}

/// One classified probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HostCheck {
    pub id: String,
    pub label: String,
    pub status: HostStatus,
    /// One sentence, always present.
    pub detail: String,
    /// Path, driver or version that explains the status.
    pub hint: Option<String>,
    /// True when [`Self::packages`] is non-empty for the detected family.
    pub installable: bool,
    /// Exact package names for the detected family.
    pub packages: Vec<String>,
    /// Official instruction when the family has no package.
    pub manual: Option<HostManual>,
}

/// Persisted probe result (`settings.json` → `hostPreflight`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HostReport {
    /// RFC 3339 UTC.
    pub checked_at: String,
    /// False outside Linux; every finding is then `unknown`.
    pub supported: bool,
    /// No `missing` and no `tooOld`.
    pub ready: bool,
    pub family: HostFamily,
    pub findings: Vec<HostCheck>,
    pub warnings: Vec<String>,
    pub duration_ms: u64,
}

/* -------------------------------- recipes -------------------------------- */

/// One row of the data table: package names per family plus the manual
/// instruction. A family with an empty list is intentionally manual.
pub struct Recipe {
    finding: &'static str,
    pacman: &'static [&'static str],
    dnf: &'static [&'static str],
    apt: &'static [&'static str],
    zypper: &'static [&'static str],
    manual: &'static str,
    url: Option<&'static str>,
}

/// The package table. One row per installable finding; the GPU row does not
/// exist on purpose and `proton` is documented as a manual step because Proton
/// builds are not distribution packages.
///
/// Sources: UMU release page, GameMode/MangoHud/Gamescope upstream READMEs and
/// the Tauri Linux prerequisite list (WebKitGTK 4.1).
pub const RECIPES: &[Recipe] = &[
    Recipe {
        finding: FINDING_UMU,
        pacman: &["umu-launcher"],
        dnf: &["umu-launcher"],
        // Ubuntu and Debian do not ship UMU: do not invent a package name.
        apt: &[],
        zypper: &["umu-launcher"],
        manual: "Ubuntu and Debian do not package UMU. Download the latest release, copy umu-run to ~/.local/bin and make it executable, or install the Flatpak.",
        url: Some("https://github.com/Open-Wine-Components/umu-launcher/releases"),
    },
    Recipe {
        finding: FINDING_GAMEMODE,
        pacman: &["gamemode"],
        dnf: &["gamemode"],
        apt: &["gamemode"],
        zypper: &["gamemode"],
        manual: "Install GameMode through your distribution or its upstream build instructions.",
        url: Some("https://github.com/FeralInteractive/gamemode"),
    },
    Recipe {
        finding: FINDING_MANGOHUD,
        pacman: &["mangohud"],
        dnf: &["mangohud"],
        apt: &["mangohud"],
        zypper: &["mangohud"],
        manual: "Install MangoHud through your distribution or its upstream build instructions.",
        url: Some("https://github.com/flightlessmango/MangoHud"),
    },
    Recipe {
        finding: FINDING_GAMESCOPE,
        pacman: &["gamescope"],
        dnf: &["gamescope"],
        apt: &["gamescope"],
        zypper: &["gamescope"],
        manual: "Install Gamescope through your distribution or its upstream build instructions.",
        url: Some("https://github.com/ValveSoftware/gamescope"),
    },
    Recipe {
        finding: FINDING_WEBKIT,
        pacman: &["webkit2gtk-4.1"],
        dnf: &["webkit2gtk4.1"],
        apt: &["libwebkit2gtk-4.1-0"],
        zypper: &["webkit2gtk4.1"],
        manual: "Install the WebKitGTK 4.1 runtime through your distribution.",
        url: Some("https://webkitgtk.org/"),
    },
];

fn recipe_for(finding: &str) -> Option<&'static Recipe> {
    RECIPES.iter().find(|recipe| recipe.finding == finding)
}

/* ------------------------------ probe input ------------------------------ */

/// One bounded command result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub success: bool,
    pub text: String,
}

/// Everything a probe reads. Real detection fills it from the host; tests fill
/// it from fixture trees.
#[derive(Debug, Clone)]
pub struct ProbeEnv {
    pub supported: bool,
    /// `/etc/os-release`-shaped file.
    pub os_release: Option<PathBuf>,
    /// `/sys/class/drm`.
    pub drm_dir: PathBuf,
    /// `/usr/share/vulkan/icd.d` and friends.
    pub vulkan_icd_dirs: Vec<PathBuf>,
    /// 64-bit library folders.
    pub lib_dirs: Vec<PathBuf>,
    /// 32-bit library folders (`lib32`, `i386-linux-gnu`, …).
    pub lib32_dirs: Vec<PathBuf>,
    /// `PATH`-shaped folders.
    pub path_dirs: Vec<PathBuf>,
    /// Reused compatibility scan (#90) for UMU and Proton.
    pub compat: Option<CompatScan>,
    pub lspci: Option<ToolOutput>,
    pub nvidia_smi: Option<ToolOutput>,
    pub vulkaninfo: Option<ToolOutput>,
}

impl ProbeEnv {
    /// Real host, using the saved compat overrides for UMU/Proton.
    pub fn detect(settings: &LocalSettings) -> Self {
        let supported = cfg!(all(unix, not(target_os = "macos")));
        let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        let (lspci, nvidia_smi, vulkaninfo) = if supported {
            (
                find_tool(&path_dirs, "lspci").and_then(|tool| run_bounded(&tool, &["-nn"])),
                find_tool(&path_dirs, "nvidia-smi").and_then(|tool| {
                    run_bounded(
                        &tool,
                        &["--query-gpu=name,driver_version", "--format=csv,noheader"],
                    )
                }),
                find_tool(&path_dirs, "vulkaninfo")
                    .and_then(|tool| run_bounded(&tool, &["--summary"])),
            )
        } else {
            (None, None, None)
        };

        let mut lib_dirs: Vec<PathBuf> = [
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
            "/usr/local/lib",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let mut lib32_dirs: Vec<PathBuf> = [
            "/usr/lib32",
            "/usr/lib/i386-linux-gnu",
            "/usr/lib/i686-linux-gnu",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        lib_dirs.retain(|dir| dir.is_dir());
        lib32_dirs.retain(|dir| dir.is_dir());

        Self {
            supported,
            os_release: Some(PathBuf::from("/etc/os-release")),
            drm_dir: PathBuf::from("/sys/class/drm"),
            vulkan_icd_dirs: [
                "/usr/share/vulkan/icd.d",
                "/usr/local/share/vulkan/icd.d",
                "/etc/vulkan/icd.d",
            ]
            .iter()
            .map(PathBuf::from)
            .collect(),
            lib_dirs,
            lib32_dirs,
            path_dirs,
            compat: Some(compat::scan(settings)),
            lspci,
            nvidia_smi,
            vulkaninfo,
        }
    }
}

/* ------------------------------- detection ------------------------------- */

/// Classify every probe once.
pub fn detect(settings: &LocalSettings) -> HostReport {
    detect_with(&ProbeEnv::detect(settings))
}

/// Classify probes against an explicit environment.
pub fn detect_with(env: &ProbeEnv) -> HostReport {
    let started = Instant::now();
    let family = detect_family(env);
    let mut warnings = Vec::new();

    let findings: Vec<HostCheck> = if env.supported {
        FINDING_ORDER
            .iter()
            .map(|id| probe(env, &family, id))
            .collect()
    } else {
        FINDING_ORDER
            .iter()
            .map(|id| HostCheck {
                id: id.to_string(),
                label: label_of(id).to_string(),
                status: HostStatus::Unknown,
                detail: "Host checks only apply to Linux; nothing was probed on this system."
                    .to_string(),
                hint: None,
                installable: false,
                packages: Vec::new(),
                manual: None,
            })
            .collect()
    };

    if family.id == PackageFamily::Unknown && env.supported {
        warnings.push(
            "The package manager could not be identified. Install the missing pieces with your distribution's tools."
                .to_string(),
        );
    }

    let ready = findings
        .iter()
        .all(|finding| !matches!(finding.status, HostStatus::Missing | HostStatus::TooOld));

    HostReport {
        checked_at: crate::util::now_rfc3339(),
        supported: env.supported,
        ready,
        family,
        findings,
        warnings,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

fn label_of(id: &str) -> &'static str {
    match id {
        FINDING_GPU => "Graphics driver",
        FINDING_VULKAN => "Vulkan",
        FINDING_VULKAN32 => "32-bit Vulkan",
        FINDING_UMU => "UMU",
        FINDING_PROTON => "Proton builds",
        FINDING_GAMEMODE => "GameMode",
        FINDING_MANGOHUD => "MangoHud",
        FINDING_GAMESCOPE => "Gamescope",
        FINDING_WEBKIT => "WebKitGTK runtime",
        _ => "Host check",
    }
}

fn probe(env: &ProbeEnv, family: &HostFamily, id: &str) -> HostCheck {
    let mut check = match id {
        FINDING_GPU => probe_gpu(env),
        FINDING_VULKAN => probe_vulkan(env),
        FINDING_VULKAN32 => probe_vulkan32(env),
        FINDING_WEBKIT => probe_webkit(env),
        FINDING_UMU => probe_umu(env),
        FINDING_PROTON => probe_proton(env),
        FINDING_GAMEMODE => probe_binary(env, FINDING_GAMEMODE, "gamemoderun"),
        FINDING_MANGOHUD => probe_mangohud(env),
        FINDING_GAMESCOPE => probe_binary(env, FINDING_GAMESCOPE, "gamescope"),
        _ => HostCheck {
            id: id.to_string(),
            label: label_of(id).to_string(),
            status: HostStatus::Unknown,
            detail: "This check is not implemented.".to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
    };
    check.id = id.to_string();
    check.label = label_of(id).to_string();
    attach_recipe(&mut check, family);
    check
}

/// Add the family's packages or the manual instruction to one finding.
fn attach_recipe(check: &mut HostCheck, family: &HostFamily) {
    let Some(recipe) = recipe_for(&check.id) else {
        check.installable = false;
        return;
    };
    if !matches!(check.status, HostStatus::Missing | HostStatus::TooOld) {
        check.installable = false;
        return;
    }
    match family.id.packages(recipe) {
        Some(packages) => {
            check.installable = true;
            check.packages = packages.iter().map(|name| (*name).to_string()).collect();
        }
        None => {
            check.installable = false;
            check.packages.clear();
            check.manual = Some(HostManual {
                instruction: recipe.manual.to_string(),
                url: recipe.url.map(str::to_string),
            });
        }
    }
}

/* --------------------------------- GPU ---------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq)]
struct DrmCard {
    name: String,
    driver: Option<String>,
    vendor: Option<String>,
}

fn probe_gpu(env: &ProbeEnv) -> HostCheck {
    let cards = read_drm_cards(&env.drm_dir);
    let mut hint = cards
        .iter()
        .filter_map(|card| card.driver.clone())
        .collect::<Vec<_>>()
        .join(" · ");
    if let Some(output) = &env.nvidia_smi {
        if output.success {
            let first = output.text.lines().next().unwrap_or("").trim();
            if !first.is_empty() {
                hint = if hint.is_empty() {
                    format!("nvidia-smi: {first}")
                } else {
                    format!("{hint} · nvidia-smi: {first}")
                };
            }
        }
    }

    if cards.is_empty() {
        if let Some(display) = display_controller(&env.lspci) {
            return HostCheck {
                id: FINDING_GPU.to_string(),
                label: label_of(FINDING_GPU).to_string(),
                status: HostStatus::Missing,
                detail: format!(
                    "{display} is present, but no kernel driver is bound to it, so no graphics API is available."
                ),
                hint: Some("lspci -nn".to_string()),
                installable: false,
                packages: Vec::new(),
                manual: Some(driver_manual()),
            };
        }
        return HostCheck {
            id: FINDING_GPU.to_string(),
            label: label_of(FINDING_GPU).to_string(),
            status: HostStatus::Unknown,
            detail:
                "No graphics device was found. A virtual machine needs a GPU or Vulkan passthrough before games can render."
                    .to_string(),
            hint: (!hint.is_empty()).then_some(hint),
            installable: false,
            packages: Vec::new(),
            manual: Some(driver_manual()),
        };
    }

    let drivers: Vec<&str> = cards
        .iter()
        .filter_map(|card| card.driver.as_deref())
        .collect();
    let has_nouveau = drivers.contains(&"nouveau");
    let fallback_only = !drivers.is_empty()
        && drivers.iter().all(|driver| {
            matches!(
                *driver,
                "simpledrm" | "efifb" | "vesa" | "vga16fb" | "ofdrm" | "udl"
            )
        });

    if has_nouveau {
        return HostCheck {
            id: FINDING_GPU.to_string(),
            label: label_of(FINDING_GPU).to_string(),
            status: HostStatus::TooOld,
            detail: "The reverse-engineered nouveau driver is loaded. NVIDIA's Vulkan driver is not available through it, so install the proprietary NVIDIA driver from your distribution."
                .to_string(),
            hint: (!hint.is_empty()).then_some(hint),
            installable: false,
            packages: Vec::new(),
            manual: Some(driver_manual()),
        };
    }
    if fallback_only {
        return HostCheck {
            id: FINDING_GPU.to_string(),
            label: label_of(FINDING_GPU).to_string(),
            status: HostStatus::Missing,
            detail:
                "Only the firmware framebuffer is active; no real GPU driver is loaded, so games cannot use Vulkan or OpenGL."
                    .to_string(),
            hint: (!hint.is_empty()).then_some(hint),
            installable: false,
            packages: Vec::new(),
            manual: Some(driver_manual()),
        };
    }

    let vendors: Vec<String> = cards
        .iter()
        .filter_map(|card| card.vendor.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let vendor = if vendors.is_empty() {
        String::new()
    } else {
        format!(" (PCI vendor {})", vendors.join(", "))
    };
    HostCheck {
        id: FINDING_GPU.to_string(),
        label: label_of(FINDING_GPU).to_string(),
        status: HostStatus::Ok,
        detail: format!("Your GPU is driven by {}{}.", drivers.join(", "), vendor),
        hint: (!hint.is_empty()).then_some(hint),
        installable: false,
        packages: Vec::new(),
        manual: None,
    }
}

fn driver_manual() -> HostManual {
    HostManual {
        instruction:
            "Install or update the graphics driver from your distribution's driver tool (for example Additional Drivers on Ubuntu). Ludex never installs drivers."
                .to_string(),
        url: Some("https://github.com/lutris/docs/blob/master/InstallingDrivers.md".to_string()),
    }
}

fn read_drm_cards(dir: &Path) -> Vec<DrmCard> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("card") && !name.contains('-'))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let uevent = dir.join(&name).join("device/uevent");
            let text = std::fs::read_to_string(uevent).unwrap_or_default();
            let driver = uevent_value(&text, "DRIVER");
            let vendor = uevent_value(&text, "PCI_ID")
                .and_then(|id| id.split(':').next().map(str::to_string));
            DrmCard {
                name,
                driver,
                vendor,
            }
        })
        .collect()
}

fn uevent_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn display_controller(lspci: &Option<ToolOutput>) -> Option<String> {
    let output = lspci.as_ref()?;
    if !output.success {
        return None;
    }
    output
        .text
        .lines()
        .find(|line| {
            line.contains("VGA compatible controller")
                || line.contains("3D controller")
                || line.contains("Display controller")
        })
        .map(|line| {
            line.split(':')
                .skip(2)
                .collect::<Vec<_>>()
                .join(":")
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
}

/* -------------------------------- Vulkan -------------------------------- */

fn icd_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        found.sort();
        files.extend(found);
    }
    files
}

fn is_software_icd(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains("lvp") || name.contains("swiftshader") || name.contains("lavapipe")
}

fn is_32bit_icd(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    name.contains("i686") || name.contains(".32.") || name.ends_with("_32.json")
}

/// What an ICD manifest says about the library it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IcdLibrary {
    /// The JSON was read and parsed but carries no usable `library_path`.
    Absent,
    /// The `ICD.library_path` value, as written in the manifest.
    Path(String),
    /// The file could not be read or parsed as JSON.
    Unreadable,
}

fn icd_library(path: &Path) -> IcdLibrary {
    let Ok(text) = std::fs::read_to_string(path) else {
        return IcdLibrary::Unreadable;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return IcdLibrary::Unreadable;
    };
    match json
        .get("ICD")
        .and_then(|icd| icd.get("library_path"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(library) => IcdLibrary::Path(library.to_string()),
        None => IcdLibrary::Absent,
    }
}

/// Resolve a manifest `library_path` for the 32-bit loader (#313).
///
/// A bare soname is resolved per architecture by whichever loader opens the
/// manifest, so it is 32-bit-capable when the name exists in a 32-bit folder:
/// Arch ships `nvidia_icd.json` with `library_path: "libGLX_nvidia.so.0"` and
/// `lib32-nvidia-utils` puts that soname in `/usr/lib32`. An absolute path is
/// only usable when the file already lives inside a 32-bit folder, because the
/// loader opens it verbatim and cannot load a 64-bit ELF.
fn resolve_icd32(env: &ProbeEnv, library: &str) -> Option<PathBuf> {
    let path = Path::new(library);
    if path.is_absolute() {
        return (path.is_file() && env.lib32_dirs.iter().any(|dir| path.starts_with(dir)))
            .then(|| path.to_path_buf());
    }
    let name = path.file_name().and_then(|name| name.to_str())?;
    find_library(&env.lib32_dirs, name)
}

/// Diagnostic-only guidance for the 32-bit finding.
///
/// A package transaction is never suggested: on Arch and CachyOS the usual
/// `nvidia-dkms` recipe conflicts with the prebuilt `linux-*-nvidia-open`
/// modules the running kernel uses, and replacing them breaks a working host.
/// The finding therefore hands out checks, not commands that change packages.
fn vulkan32_manual() -> HostManual {
    HostManual {
        instruction: "Check before changing packages: run vulkaninfo --summary with a 32-bit vulkaninfo when your distribution ships one, and ldd the ICD library (for example /usr/lib32/libGLX_nvidia.so.0) to see whether it is a 32-bit build. The 32-bit loader is owned by a package your distribution can name (pacman -Qo /usr/lib32/libvulkan.so.1 on Arch, dpkg -S on Debian). If that package is missing, install only the 32-bit Vulkan packages your distribution provides; never remove or replace a driver package that already works, and never force a dependency conflict to add 32-bit support. Ludex never installs drivers."
            .to_string(),
        url: Some("https://wiki.archlinux.org/title/Vulkan".to_string()),
    }
}

fn probe_vulkan(env: &ProbeEnv) -> HostCheck {
    let icds = icd_files(&env.vulkan_icd_dirs);
    let hardware: Vec<&PathBuf> = icds.iter().filter(|path| !is_software_icd(path)).collect();
    let hint = |paths: &[&PathBuf]| {
        (!paths.is_empty()).then(|| {
            paths
                .iter()
                .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
                .collect::<Vec<_>>()
                .join(" · ")
        })
    };

    if icds.is_empty() {
        return HostCheck {
            id: FINDING_VULKAN.to_string(),
            label: label_of(FINDING_VULKAN).to_string(),
            status: HostStatus::Missing,
            detail:
                "No Vulkan driver is installed: /usr/share/vulkan/icd.d contains no ICD, so Windows games cannot use DXVK or VKD3D."
                    .to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: Some(HostManual {
                instruction:
                    "Install your GPU vendor's Vulkan driver package from your distribution (Mesa for AMD/Intel, the NVIDIA driver for NVIDIA). Ludex never installs drivers."
                        .to_string(),
                url: Some(
                    "https://github.com/lutris/docs/blob/master/InstallingDrivers.md".to_string(),
                ),
            }),
        };
    }
    if hardware.is_empty() {
        return HostCheck {
            id: FINDING_VULKAN.to_string(),
            label: label_of(FINDING_VULKAN).to_string(),
            status: HostStatus::TooOld,
            detail:
                "Only the CPU fallback Vulkan driver is installed. Games would run on software rendering at a few frames per second; install your GPU vendor's driver."
                    .to_string(),
            hint: hint(&icds.iter().collect::<Vec<_>>()),
            installable: false,
            packages: Vec::new(),
            manual: Some(driver_manual()),
        };
    }
    if let Some(output) = &env.vulkaninfo {
        if !output.success {
            return HostCheck {
                id: FINDING_VULKAN.to_string(),
                label: label_of(FINDING_VULKAN).to_string(),
                status: HostStatus::TooOld,
                detail: "The Vulkan loader is installed, but vulkaninfo fails, so no device is exposed to games."
                    .to_string(),
                hint: hint(&hardware),
                installable: false,
                packages: Vec::new(),
                manual: Some(driver_manual()),
            };
        }
    }

    HostCheck {
        id: FINDING_VULKAN.to_string(),
        label: label_of(FINDING_VULKAN).to_string(),
        status: HostStatus::Ok,
        detail: format!(
            "{} Vulkan driver{} installed.",
            hardware.len(),
            if hardware.len() == 1 { " is" } else { "s are" }
        ),
        hint: hint(&hardware),
        installable: false,
        packages: Vec::new(),
        manual: None,
    }
}

fn probe_vulkan32(env: &ProbeEnv) -> HostCheck {
    let icds = icd_files(&env.vulkan_icd_dirs);
    let loader32 = find_library(&env.lib32_dirs, "libvulkan.so.1")
        .or_else(|| find_library(&env.lib32_dirs, "libvulkan.so"));
    let Some(loader) = loader32 else {
        return HostCheck {
            id: FINDING_VULKAN32.to_string(),
            label: label_of(FINDING_VULKAN32).to_string(),
            status: HostStatus::Missing,
            detail: "The 32-bit Vulkan runtime is not installed; many Windows games are 32-bit and cannot start without it."
                .to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: Some(vulkan32_manual()),
        };
    };

    // Some vendors ship a manifest per architecture (`radeon_icd.i686.json`,
    // `*_32.json`); the file name is a direct hit when it is present.
    if let Some(icd) = icds.iter().find(|path| is_32bit_icd(path)) {
        return HostCheck {
            id: FINDING_VULKAN32.to_string(),
            label: label_of(FINDING_VULKAN32).to_string(),
            status: HostStatus::Ok,
            detail: "The 32-bit Vulkan loader and a 32-bit driver are installed; 32-bit Windows games can render."
                .to_string(),
            hint: Some(format!(
                "{} · {}",
                loader.display(),
                icd.file_name().and_then(|name| name.to_str()).unwrap_or("")
            )),
            installable: false,
            packages: Vec::new(),
            manual: None,
        };
    }

    // Otherwise parse each manifest and resolve `library_path` the way the
    // 32-bit loader would (#313): one arch-agnostic ICD serves both loaders.
    let mut readable = 0_usize;
    let mut unresolved: Option<(String, String)> = None;
    for icd in &icds {
        let name = icd
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        match icd_library(icd) {
            IcdLibrary::Unreadable => {}
            IcdLibrary::Absent => readable += 1,
            IcdLibrary::Path(library) => {
                readable += 1;
                if let Some(resolved) = resolve_icd32(env, &library) {
                    return HostCheck {
                        id: FINDING_VULKAN32.to_string(),
                        label: label_of(FINDING_VULKAN32).to_string(),
                        status: HostStatus::Ok,
                        detail: format!(
                            "The 32-bit Vulkan loader is installed; {name} serves both architectures and resolves to {}, so 32-bit Windows games can render.",
                            resolved.display()
                        ),
                        hint: Some(format!("{} · {name} → {}", loader.display(), resolved.display())),
                        installable: false,
                        packages: Vec::new(),
                        manual: None,
                    };
                }
                unresolved.get_or_insert((name, library));
            }
        }
    }

    let hint = if let Some((name, library)) = unresolved {
        format!(
            "{} · {name} → {library} (not resolved in a 32-bit library folder)",
            loader.display()
        )
    } else {
        loader.display().to_string()
    };

    if icds.is_empty() {
        return HostCheck {
            id: FINDING_VULKAN32.to_string(),
            label: label_of(FINDING_VULKAN32).to_string(),
            status: HostStatus::TooOld,
            detail: "The 32-bit Vulkan loader is installed, but no driver ICD manifest exists, so 32-bit games fall back to OpenGL."
                .to_string(),
            hint: Some(hint),
            installable: false,
            packages: Vec::new(),
            manual: Some(vulkan32_manual()),
        };
    }

    if readable == 0 {
        return HostCheck {
            id: FINDING_VULKAN32.to_string(),
            label: label_of(FINDING_VULKAN32).to_string(),
            status: HostStatus::Unknown,
            detail: "The 32-bit Vulkan loader is installed, but none of the Vulkan ICD manifests could be read, so 32-bit support could not be confirmed."
                .to_string(),
            hint: Some(hint),
            installable: false,
            packages: Vec::new(),
            manual: Some(vulkan32_manual()),
        };
    }

    HostCheck {
        id: FINDING_VULKAN32.to_string(),
        label: label_of(FINDING_VULKAN32).to_string(),
        status: HostStatus::TooOld,
        detail: "The 32-bit Vulkan loader is installed, but no 32-bit driver ICD is exposed, so 32-bit games fall back to OpenGL."
            .to_string(),
        hint: Some(hint),
        installable: false,
        packages: Vec::new(),
        manual: Some(vulkan32_manual()),
    }
}

/* -------------------------------- tools --------------------------------- */

fn probe_umu(env: &ProbeEnv) -> HostCheck {
    let from_compat = env.compat.as_ref().and_then(|scan| scan.umu.clone());
    let from_path = find_in_dirs(&env.path_dirs, "umu-run")
        .or_else(|| find_in_dirs(&env.path_dirs, "umu-launcher"))
        .map(|path| path.to_string_lossy().to_string());
    match from_compat.or(from_path) {
        Some(path) => HostCheck {
            id: FINDING_UMU.to_string(),
            label: label_of(FINDING_UMU).to_string(),
            status: HostStatus::Ok,
            detail: "UMU is installed; Ludex can run Windows games with a Proton build or a 32-bit runtime."
                .to_string(),
            hint: Some(path),
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
        None if env.compat.is_none() => HostCheck {
            id: FINDING_UMU.to_string(),
            label: label_of(FINDING_UMU).to_string(),
            status: HostStatus::Unknown,
            detail: "The compatibility scan did not run, so UMU could not be detected."
                .to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
        None => HostCheck {
            id: FINDING_UMU.to_string(),
            label: label_of(FINDING_UMU).to_string(),
            status: HostStatus::Missing,
            detail: "UMU is not installed. It is the supported way to run a Windows game with a Proton build outside Steam."
                .to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
    }
}

fn probe_proton(env: &ProbeEnv) -> HostCheck {
    let Some(scan) = &env.compat else {
        return HostCheck {
            id: FINDING_PROTON.to_string(),
            label: label_of(FINDING_PROTON).to_string(),
            status: HostStatus::Unknown,
            detail: "The compatibility scan did not run, so Proton builds could not be listed."
                .to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: None,
        };
    };
    let protons: Vec<&_> = scan
        .tools
        .iter()
        .filter(|tool| tool.kind == ToolKind::Proton)
        .collect();
    if protons.is_empty() {
        return HostCheck {
            id: FINDING_PROTON.to_string(),
            label: label_of(FINDING_PROTON).to_string(),
            status: HostStatus::Missing,
            detail: "No Proton build is installed. Use Make it run on a game to download one; Steam's compatibilitytools.d and steamapps folders are both scanned."
                .to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: Some(HostManual {
                instruction: "Open a game, choose Make it run, and let Ludex install a Proton build."
                    .to_string(),
                url: Some("https://github.com/Open-Wine-Components/umu-launcher".to_string()),
            }),
        };
    }
    let names: Vec<String> = protons
        .iter()
        .take(3)
        .map(|tool| tool.name.clone())
        .collect();
    HostCheck {
        id: FINDING_PROTON.to_string(),
        label: label_of(FINDING_PROTON).to_string(),
        status: HostStatus::Ok,
        detail: format!(
            "{} Proton build{} available{}.",
            protons.len(),
            if protons.len() == 1 { " is" } else { "s are" },
            if names.is_empty() {
                String::new()
            } else {
                format!(": {}", names.join(", "))
            }
        ),
        hint: None,
        installable: false,
        packages: Vec::new(),
        manual: None,
    }
}

fn probe_binary(env: &ProbeEnv, id: &str, name: &str) -> HostCheck {
    match find_in_dirs(&env.path_dirs, name) {
        Some(path) => HostCheck {
            id: id.to_string(),
            label: label_of(id).to_string(),
            status: HostStatus::Ok,
            detail: format!("{name} is installed."),
            hint: Some(path.to_string_lossy().to_string()),
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
        None => HostCheck {
            id: id.to_string(),
            label: label_of(id).to_string(),
            status: HostStatus::Missing,
            detail: format!("{name} is not installed."),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
    }
}

fn probe_mangohud(env: &ProbeEnv) -> HostCheck {
    let found = find_mangohud(env);
    match found {
        Some(path) => HostCheck {
            id: FINDING_MANGOHUD.to_string(),
            label: label_of(FINDING_MANGOHUD).to_string(),
            status: HostStatus::Ok,
            detail: "The MangoHud Vulkan layer is installed.".to_string(),
            hint: Some(path.to_string_lossy().to_string()),
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
        None => HostCheck {
            id: FINDING_MANGOHUD.to_string(),
            label: label_of(FINDING_MANGOHUD).to_string(),
            status: HostStatus::Missing,
            detail: "The MangoHud Vulkan layer (libMangoHud.so) is not installed, so the performance overlay cannot be shown.".to_string(),
            hint: None,
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
    }
}

fn find_mangohud(env: &ProbeEnv) -> Option<PathBuf> {
    for dir in &env.lib_dirs {
        if let Some(path) = find_library(std::slice::from_ref(dir), "libMangoHud.so") {
            return Some(path);
        }
        if let Some(path) = find_library(&[dir.join("mangohud")], "libMangoHud.so") {
            return Some(path);
        }
    }
    // 32-bit Vulkan needs its own layer.
    find_library(&env.lib32_dirs, "libMangoHud.so")
}

fn probe_webkit(env: &ProbeEnv) -> HostCheck {
    match find_library(&env.lib_dirs, "libwebkit2gtk-4.1.so.0") {
        Some(path) => HostCheck {
            id: FINDING_WEBKIT.to_string(),
            label: label_of(FINDING_WEBKIT).to_string(),
            status: HostStatus::Ok,
            detail: "The WebKitGTK 4.1 runtime the desktop app needs is installed.".to_string(),
            hint: Some(path.to_string_lossy().to_string()),
            installable: false,
            packages: Vec::new(),
            manual: None,
        },
        None => {
            let old = find_library(&env.lib_dirs, "libwebkit2gtk-4.0.so.0");
            if old.is_some() {
                HostCheck {
                    id: FINDING_WEBKIT.to_string(),
                    label: label_of(FINDING_WEBKIT).to_string(),
                    status: HostStatus::TooOld,
                    detail:
                        "Only the WebKitGTK 4.0 runtime is installed; this Ludex build needs 4.1."
                            .to_string(),
                    hint: old.map(|path| path.to_string_lossy().to_string()),
                    installable: false,
                    packages: Vec::new(),
                    manual: None,
                }
            } else {
                HostCheck {
                    id: FINDING_WEBKIT.to_string(),
                    label: label_of(FINDING_WEBKIT).to_string(),
                    status: HostStatus::Missing,
                    detail: "The WebKitGTK 4.1 runtime (libwebkit2gtk-4.1.so.0) is not installed; the AppImage cannot start without it."
                        .to_string(),
                    hint: None,
                    installable: false,
                    packages: Vec::new(),
                    manual: None,
                }
            }
        }
    }
}

/* --------------------------- family detection ---------------------------- */

/// Read `/etc/os-release` and the package manager binaries.
pub fn detect_family(env: &ProbeEnv) -> HostFamily {
    let os_release = env
        .os_release
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let id = os_release_value(&os_release, "ID").unwrap_or_default();
    let id_like = os_release_value(&os_release, "ID_LIKE").unwrap_or_default();
    let tokens: Vec<String> = std::iter::once(id.to_ascii_lowercase())
        .chain(
            id_like
                .split_whitespace()
                .map(|token| token.to_ascii_lowercase()),
        )
        .filter(|token| !token.is_empty())
        .collect();

    let known = |family: PackageFamily, names: &[&str]| {
        if tokens.iter().any(|token| names.contains(&token.as_str())) {
            Some(family)
        } else {
            None
        }
    };

    let from_id = known(
        PackageFamily::Pacman,
        &[
            "arch",
            "archlinux",
            "endeavouros",
            "cachyos",
            "manjaro",
            "garuda",
            "artix",
        ],
    )
    .or_else(|| {
        known(
            PackageFamily::Dnf,
            &[
                "fedora",
                "nobara",
                "bazzite",
                "centos",
                "rhel",
                "rocky",
                "almalinux",
            ],
        )
    })
    .or_else(|| {
        known(
            PackageFamily::Apt,
            &[
                "debian",
                "ubuntu",
                "linuxmint",
                "pop",
                "zorin",
                "elementary",
                "kali",
                "raspbian",
                "neon",
            ],
        )
    })
    .or_else(|| {
        known(
            PackageFamily::Zypper,
            &[
                "opensuse",
                "opensuse-tumbleweed",
                "opensuse-leap",
                "suse",
                "sles",
                "sled",
            ],
        )
    });

    if let Some(family) = from_id {
        return family_of(
            family,
            format!(
                "ID={}{} in /etc/os-release",
                if id.is_empty() { "?" } else { id.as_str() },
                if id_like.is_empty() {
                    String::new()
                } else {
                    format!(", ID_LIKE={id_like}")
                }
            ),
        );
    }

    // Unknown or missing os-release: fall back to whichever package manager is
    // actually installed.
    let by_binary = [
        (PackageFamily::Pacman, "pacman"),
        (PackageFamily::Dnf, "dnf"),
        (PackageFamily::Apt, "apt"),
        (PackageFamily::Zypper, "zypper"),
    ]
    .into_iter()
    .find(|(_, binary)| find_in_dirs(&env.path_dirs, binary).is_some());

    match by_binary {
        Some((family, binary)) => {
            family_of(family, format!("the {binary} binary is on PATH"))
        }
        None => HostFamily {
            id: PackageFamily::Unknown,
            label: "Unknown distribution".to_string(),
            detail: "Neither /etc/os-release nor PATH names pacman, dnf, apt or zypper; install the missing pieces with your distribution's tools."
                .to_string(),
            installable: false,
        },
    }
}

fn family_of(id: PackageFamily, detail: String) -> HostFamily {
    let label = match id {
        PackageFamily::Pacman => "Arch Linux and derivatives (pacman)",
        PackageFamily::Dnf => "Fedora and derivatives (dnf)",
        PackageFamily::Apt => "Debian and Ubuntu (apt)",
        PackageFamily::Zypper => "openSUSE and SUSE (zypper)",
        PackageFamily::Unknown => "Unknown distribution",
    };
    HostFamily {
        id,
        label: label.to_string(),
        detail,
        installable: id != PackageFamily::Unknown,
    }
}

fn os_release_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.strip_prefix(&format!("{key}="))
            .map(|value| value.trim().trim_matches('"').to_string())
    })
}

/* ----------------------------- install plan ----------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPackage {
    pub finding: String,
    pub label: String,
    pub package: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostManualStep {
    pub finding: String,
    pub label: String,
    pub instruction: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRefusal {
    pub finding: String,
    pub reason: String,
}

/// What would run, exactly. Building it never executes anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInstallPlan {
    pub family: HostFamily,
    /// Full argv including `pkexec`; empty when nothing can be installed.
    pub command: Vec<String>,
    /// The same command as one shell-ish line, for the consent screen.
    pub command_label: String,
    pub packages: Vec<HostPackage>,
    pub manual: Vec<HostManualStep>,
    pub refused: Vec<HostRefusal>,
    pub notes: Vec<String>,
}

fn install_argv(family: PackageFamily, packages: &[String]) -> Option<Vec<String>> {
    if packages.is_empty() {
        return None;
    }
    let base: &[&str] = match family {
        PackageFamily::Pacman => &["pkexec", "pacman", "-S", "--needed", "--noconfirm"],
        PackageFamily::Dnf => &["pkexec", "dnf", "install", "-y"],
        PackageFamily::Apt => &["pkexec", "apt-get", "install", "-y"],
        PackageFamily::Zypper => &["pkexec", "zypper", "--non-interactive", "install"],
        PackageFamily::Unknown => return None,
    };
    let mut argv: Vec<String> = base.iter().map(|part| (*part).to_string()).collect();
    argv.extend(packages.iter().cloned());
    Some(argv)
}

/// Build the plan for the requested finding ids against a recorded report.
///
/// The GPU finding is refused by name; anything without a recipe row is
/// refused with the reason. Nothing outside [`RECIPES`] can ever be installed.
pub fn plan_install(report: &HostReport, requested: &[String]) -> HostInstallPlan {
    let mut packages: Vec<HostPackage> = Vec::new();
    let mut manual: Vec<HostManualStep> = Vec::new();
    let mut refused: Vec<HostRefusal> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let mut requested: Vec<String> = requested
        .iter()
        .map(|id| id.trim().to_ascii_lowercase())
        .filter(|id| !id.is_empty())
        .collect();
    requested.sort();
    requested.dedup();

    for id in &requested {
        if id == FINDING_GPU {
            refused.push(HostRefusal {
                finding: id.clone(),
                reason:
                    "Ludex never installs GPU drivers. Update them from your distribution's driver tool."
                        .to_string(),
            });
            continue;
        }
        let Some(recipe) = recipe_for(id) else {
            refused.push(HostRefusal {
                finding: id.clone(),
                reason: "This check has no package recipe.".to_string(),
            });
            continue;
        };
        let finding = report.findings.iter().find(|finding| &finding.id == id);
        let label = finding
            .map(|finding| finding.label.clone())
            .unwrap_or_else(|| label_of(id).to_string());
        match report.family.id.packages(recipe) {
            Some(list) => {
                for package in list {
                    if seen.insert((*package).to_string()) {
                        packages.push(HostPackage {
                            finding: id.clone(),
                            label: label.clone(),
                            package: (*package).to_string(),
                        });
                    }
                }
            }
            None => {
                if let Some(finding) = finding {
                    if let Some(step) = &finding.manual {
                        manual.push(HostManualStep {
                            finding: id.clone(),
                            label: label.clone(),
                            instruction: step.instruction.clone(),
                            url: step.url.clone(),
                        });
                    }
                }
            }
        }
    }

    if !requested.is_empty() && packages.is_empty() && manual.is_empty() && refused.is_empty() {
        notes.push("Nothing was selected that can be installed.".to_string());
    }
    if report.family.id == PackageFamily::Unknown {
        notes.push(
            "The package manager could not be identified, so no install command was built."
                .to_string(),
        );
    }

    let package_names: Vec<String> = packages
        .iter()
        .map(|package| package.package.clone())
        .collect();
    let command = install_argv(report.family.id, &package_names).unwrap_or_default();
    let command_label = command.join(" ");

    HostInstallPlan {
        family: report.family.clone(),
        command,
        command_label,
        packages,
        manual,
        refused,
        notes,
    }
}

/// Installing needs the explicit consent screen; a dry run does not.
pub fn install_guard(dry_run: bool, consent: bool) -> Result<(), String> {
    if dry_run {
        return Ok(());
    }
    if !consent {
        return Err(
            "Installing packages requires the consent screen listing the exact packages."
                .to_string(),
        );
    }
    Ok(())
}

/// Result of one install command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInstallReport {
    pub success: bool,
    /// True when pkexec reported a dismissal (exit 126/127).
    pub cancelled: bool,
    pub exit_code: Option<i32>,
    pub output: String,
    pub command: Vec<String>,
    pub duration_ms: u64,
}

/// Plan plus (unless dry run) the execution result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInstallOutcome {
    pub plan: HostInstallPlan,
    pub report: Option<HostInstallReport>,
}

/* ----------------------------- commands --------------------------------- */

fn state_settings(app: &AppHandle) -> LocalSettings {
    app.state::<AppState>().settings_snapshot()
}

async fn report_for(app: &AppHandle, refresh: bool) -> Result<HostReport, String> {
    if !refresh {
        if let Some(report) = state_settings(app).host_preflight {
            return Ok(report);
        }
    }
    let settings = state_settings(app);
    let report = tokio::task::spawn_blocking(move || detect(&settings))
        .await
        .map_err(|error| format!("The host checks failed: {error}"))?;
    if let Err(error) = app
        .state::<AppState>()
        .update_settings(|settings| settings.host_preflight = Some(report.clone()))
    {
        tracing::warn!(error = %error, "the host preflight result could not be saved");
    }
    Ok(report)
}

/// The persisted host report, or a fresh probe when nothing was stored yet.
/// `refresh` is the manual "Re-run checks": it probes again and replaces it.
#[tauri::command]
pub async fn host_preflight_report(app: AppHandle, refresh: bool) -> Result<HostReport, String> {
    report_for(&app, refresh).await
}

/// Dry run: the exact command the consent screen would show.
#[tauri::command]
pub async fn host_preflight_plan(
    app: AppHandle,
    findings: Vec<String>,
) -> Result<HostInstallPlan, String> {
    let report = report_for(&app, false).await?;
    Ok(plan_install(&report, &findings))
}

/// Install the selected findings after consent, or only describe the command
/// with `dryRun`. A successful install clears the stored report so the next
/// launch probes again instead of claiming the old gaps are still there.
#[tauri::command]
pub async fn host_preflight_install(
    app: AppHandle,
    findings: Vec<String>,
    dry_run: bool,
    consent: bool,
) -> Result<HostInstallOutcome, String> {
    install_guard(dry_run, consent)?;
    let report = report_for(&app, false).await?;
    let plan = plan_install(&report, &findings);
    if dry_run {
        return Ok(HostInstallOutcome { plan, report: None });
    }
    if plan.command.is_empty() {
        return Err("There is nothing to install for the selected checks.".to_string());
    }

    let command = plan.command.clone();
    let result = tokio::task::spawn_blocking(move || run_install(&command))
        .await
        .map_err(|error| format!("The install command failed to start: {error}"))?;

    if result.success {
        if let Err(error) = app
            .state::<AppState>()
            .update_settings(|settings| settings.host_preflight = None)
        {
            tracing::warn!(error = %error, "the host preflight result could not be cleared");
        }
    }
    Ok(HostInstallOutcome {
        plan,
        report: Some(result),
    })
}

/// Best effort: first start records a report so the launcher does not probe on
/// every launch. Runs off the main thread and never fails startup.
pub fn spawn_startup_probe(app: AppHandle) {
    if !cfg!(all(unix, not(target_os = "macos"))) {
        return;
    }
    if app
        .state::<AppState>()
        .settings_snapshot()
        .host_preflight
        .is_some()
    {
        return;
    }
    std::thread::spawn(move || {
        let settings = app.state::<AppState>().settings_snapshot();
        let report = detect(&settings);
        if let Err(error) = app
            .state::<AppState>()
            .update_settings(|settings| settings.host_preflight = Some(report))
        {
            tracing::warn!(error = %error, "the startup host preflight could not be saved");
        }
    });
}

fn run_install(argv: &[String]) -> HostInstallReport {
    let started = Instant::now();
    let mut command = crate::process::capture(&argv[0]);
    command.args(&argv[1..]);
    let output = run_command(command, INSTALL_TIMEOUT);
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match output {
        Some(output) => {
            let code = output.exit_code;
            HostInstallReport {
                success: code == Some(0),
                cancelled: matches!(code, Some(126) | Some(127)),
                exit_code: code,
                output: truncate(&output.text, MAX_OUTPUT_CHARS),
                command: argv.to_vec(),
                duration_ms,
            }
        }
        None => HostInstallReport {
            success: false,
            cancelled: false,
            exit_code: None,
            output: "The command did not finish within 20 minutes, or pkexec is not installed."
                .to_string(),
            command: argv.to_vec(),
            duration_ms,
        },
    }
}

/* ------------------------------- helpers -------------------------------- */

struct Completed {
    exit_code: Option<i32>,
    text: String,
}

/// Run one command, bounded. Returns `None` on spawn failure or timeout.
///
/// Bounded queues are large enough that any command might fill a 64 KiB pipe
/// before it exits, so stdout and stderr are drained on their own threads while
/// the child runs; the timeout then kills and reaps it.
fn run_command(mut command: std::process::Command, timeout: Duration) -> Option<Completed> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read_out = std::thread::spawn(move || read_all(stdout));
    let read_err = std::thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break None,
        }
    };
    let mut text = read_out.join().unwrap_or_default();
    let errors = read_err.join().unwrap_or_default();
    let status = status?;
    if !errors.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&errors);
    }
    Some(Completed {
        exit_code: status.code(),
        text,
    })
}

fn read_all(reader: Option<impl Read>) -> String {
    let mut text = String::new();
    if let Some(mut reader) = reader {
        let _ = reader.read_to_string(&mut text);
    }
    text
}

/// A bounded probe command.
fn run_bounded(program: &Path, args: &[&str]) -> Option<ToolOutput> {
    let mut command = crate::process::capture(program);
    command.args(args);
    let completed = run_command(command, PROBE_TIMEOUT)?;
    Some(ToolOutput {
        success: completed.exit_code == Some(0),
        text: completed.text,
    })
}

fn find_tool(path_dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    find_in_dirs(path_dirs, name).or_else(|| {
        [
            "/usr/bin",
            "/usr/sbin",
            "/usr/local/bin",
            "/bin",
            "/sbin",
            "/usr/games",
        ]
        .iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|path| path.is_file())
    })
}

fn find_in_dirs(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn find_library(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    find_in_dirs(dirs, name)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(max).collect();
    kept.push_str("\n… (truncated)");
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{CompatOverrides, CompatScan, CompatTool, ScanPaths};
    use crate::linux_gaming::LaunchMode;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-host-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn empty_compat() -> CompatScan {
        CompatScan {
            platform: "linux".to_string(),
            linux: true,
            steam_roots: Vec::new(),
            tools: Vec::new(),
            umu: None,
            proton: None,
            wine: None,
            winetricks: None,
            gamescope: None,
            protonfixes: None,
            default_mode: LaunchMode::Auto,
            steam_client_path: None,
            suggested_verbs: Vec::new(),
            warnings: Vec::new(),
            display_capabilities: crate::display_caps::DisplayCapabilities::default(),
        }
    }

    fn compat_with_umu_and_proton() -> CompatScan {
        let mut scan = empty_compat();
        scan.tools = vec![
            CompatTool {
                id: "umu:/usr/bin/umu-run".to_string(),
                name: "umu-run".to_string(),
                kind: ToolKind::Umu,
                version: None,
                path: "/usr/bin/umu-run".to_string(),
                proton_path: None,
                source: "path".to_string(),
                steam_root: None,
            },
            CompatTool {
                id: "proton:/games/GE-Proton9-20".to_string(),
                name: "GE-Proton9-20".to_string(),
                kind: ToolKind::Proton,
                version: Some("GE-Proton9-20".to_string()),
                path: "/games/GE-Proton9-20/proton".to_string(),
                proton_path: Some("/games/GE-Proton9-20".to_string()),
                source: "compatibilitytools.d".to_string(),
                steam_root: None,
            },
        ];
        scan.umu = Some("/usr/bin/umu-run".to_string());
        scan.proton = Some("/games/GE-Proton9-20/proton".to_string());
        scan
    }

    /// A fixture tree: everything under one temp root, nothing on the host.
    fn base_env(root: &Path) -> ProbeEnv {
        ProbeEnv {
            supported: true,
            os_release: Some(root.join("etc/os-release")),
            drm_dir: root.join("sys/class/drm"),
            vulkan_icd_dirs: vec![root.join("usr/share/vulkan/icd.d")],
            lib_dirs: vec![root.join("usr/lib")],
            lib32_dirs: vec![root.join("usr/lib32")],
            path_dirs: vec![root.join("usr/bin")],
            compat: Some(empty_compat()),
            lspci: None,
            nvidia_smi: None,
            vulkaninfo: None,
        }
    }

    fn finding<'a>(report: &'a HostReport, id: &str) -> &'a HostCheck {
        report
            .findings
            .iter()
            .find(|finding| finding.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from the report"))
    }

    fn os_release(root: &Path, text: &str) {
        write(&root.join("etc/os-release"), text);
    }

    fn card(root: &Path, name: &str, driver: &str, vendor: &str) {
        write(
            &root.join(format!("sys/class/drm/{name}/device/uevent")),
            &format!("DRIVER={driver}\nPCI_CLASS=30000\nPCI_ID={vendor}:1234\n"),
        );
    }

    #[test]
    fn gpu_with_a_bound_driver_is_ok() {
        let root = temp_dir("gpu-ok");
        let env = base_env(&root);
        card(&root, "card0", "i915", "8086");

        let check = probe_gpu(&env);
        assert_eq!(check.status, HostStatus::Ok);
        assert!(check.detail.contains("i915"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn nouveau_is_reported_as_too_old_and_never_installed() {
        let root = temp_dir("gpu-nouveau");
        let env = base_env(&root);
        os_release(&root, "ID=ubuntu\n");
        card(&root, "card0", "nouveau", "10DE");

        let check = probe_gpu(&env);
        assert_eq!(check.status, HostStatus::TooOld);
        assert!(check.detail.to_ascii_lowercase().contains("nouveau"));
        let mut report = detect_with(&env);
        report.family = family_of(PackageFamily::Apt, String::new());
        let plan = plan_install(&report, &[FINDING_GPU.to_string()]);
        assert!(plan.command.is_empty());
        assert!(plan
            .refused
            .iter()
            .any(|refusal| refusal.reason.contains("never installs GPU drivers")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn firmware_framebuffer_is_a_missing_driver() {
        let root = temp_dir("gpu-simpledrm");
        let env = base_env(&root);
        card(&root, "card0", "simpledrm", "1234");

        assert_eq!(probe_gpu(&env).status, HostStatus::Missing);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn gpu_present_in_lspci_without_a_driver_is_missing() {
        let root = temp_dir("gpu-lspci");
        let mut env = base_env(&root);
        env.lspci = Some(ToolOutput {
            success: true,
            text: "01:00.0 VGA compatible controller [0300]: NVIDIA Corporation AD104 [GeForce RTX 4070] [10de:2786]\n".to_string(),
        });

        let check = probe_gpu(&env);
        assert_eq!(check.status, HostStatus::Missing);
        assert!(check.detail.contains("GeForce RTX 4070"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_gpu_at_all_is_unknown_not_an_error() {
        let root = temp_dir("gpu-none");
        let env = base_env(&root);
        assert_eq!(probe_gpu(&env).status, HostStatus::Unknown);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_icds_classify_ok_too_old_and_missing() {
        let root = temp_dir("vulkan");

        let mut ok = base_env(&root);
        write(
            &root.join("usr/share/vulkan/icd.d/intel_icd.x86_64.json"),
            "{}",
        );
        assert_eq!(probe_vulkan(&ok).status, HostStatus::Ok);

        let old = root.join("usr/share/vulkan/icd.d");
        std::fs::remove_dir_all(&old).unwrap();
        write(
            &root.join("usr/share/vulkan/icd.d/lvp_icd.x86_64.json"),
            "{}",
        );
        assert_eq!(probe_vulkan(&ok).status, HostStatus::TooOld);

        std::fs::remove_dir_all(root.join("usr/share/vulkan/icd.d")).unwrap();
        assert_eq!(probe_vulkan(&ok).status, HostStatus::Missing);

        // A failing vulkaninfo downgrades a present driver.
        ok.vulkaninfo = Some(ToolOutput {
            success: false,
            text: "ERROR: vkEnumeratePhysicalDevices".to_string(),
        });
        write(
            &root.join("usr/share/vulkan/icd.d/radeon_icd.x86_64.json"),
            "{}",
        );
        assert_eq!(probe_vulkan(&ok).status, HostStatus::TooOld);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_32bit_needs_both_the_loader_and_a_32bit_icd() {
        let root = temp_dir("vulkan32");
        let env = base_env(&root);
        write(
            &root.join("usr/share/vulkan/icd.d/intel_icd.x86_64.json"),
            "{}",
        );
        assert_eq!(probe_vulkan32(&env).status, HostStatus::Missing);

        write(&root.join("usr/lib32/libvulkan.so.1"), "");
        let check = probe_vulkan32(&env);
        assert_eq!(
            check.status,
            HostStatus::TooOld,
            "loader without a 32-bit ICD"
        );

        write(
            &root.join("usr/share/vulkan/icd.d/intel_icd.i686.json"),
            "{}",
        );
        assert_eq!(probe_vulkan32(&env).status, HostStatus::Ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_32bit_accepts_an_arch_agnostic_nvidia_icd() {
        let root = temp_dir("vulkan32-nvidia");
        let env = base_env(&root);
        // Arch ships exactly one NVIDIA manifest: a bare soname that the
        // loader resolves per architecture (#313).
        write(
            &root.join("usr/share/vulkan/icd.d/nvidia_icd.json"),
            r#"{"file_format_version":"1.0.1","ICD":{"library_path":"libGLX_nvidia.so.0","api_version":"1.4.357"}}"#,
        );
        write(&root.join("usr/lib32/libvulkan.so.1"), "");
        write(&root.join("usr/lib32/libGLX_nvidia.so.0"), "");

        let check = probe_vulkan32(&env);
        assert_eq!(check.status, HostStatus::Ok, "{}", check.detail);
        assert!(check.detail.contains("serves both architectures"));
        let hint = check.hint.unwrap_or_default();
        assert!(hint.contains("nvidia_icd.json"), "{hint}");
        assert!(
            hint.contains(&*root.join("usr/lib32/libGLX_nvidia.so.0").to_string_lossy()),
            "{hint}"
        );

        // Without lib32-nvidia-utils the bare soname does not resolve.
        std::fs::remove_file(root.join("usr/lib32/libGLX_nvidia.so.0")).unwrap();
        let check = probe_vulkan32(&env);
        assert_eq!(check.status, HostStatus::TooOld);
        assert!(check.hint.unwrap_or_default().contains("not resolved"));
        let manual = check
            .manual
            .expect("the 32-bit finding keeps a manual step");
        assert!(manual.instruction.contains("vulkaninfo --summary"));
        assert!(!manual.instruction.to_ascii_lowercase().contains("dkms"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_32bit_accepts_a_manifest_inside_the_i386_folder() {
        let root = temp_dir("vulkan32-i386");
        let mut env = base_env(&root);
        let i386 = root.join("usr/lib/i386-linux-gnu");
        env.lib32_dirs = vec![root.join("usr/lib32"), i386.clone()];
        write(&i386.join("libvulkan.so.1"), "");
        write(&i386.join("libvulkan_radeon.so"), "");
        write(
            &root.join("usr/share/vulkan/icd.d/radeon_icd.json"),
            &format!(
                r#"{{"ICD":{{"library_path":"{}"}}}}"#,
                i386.join("libvulkan_radeon.so").display()
            ),
        );

        assert_eq!(probe_vulkan32(&env).status, HostStatus::Ok);

        // A vendor manifest that names the architecture is still a direct hit.
        std::fs::remove_file(root.join("usr/share/vulkan/icd.d/radeon_icd.json")).unwrap();
        write(
            &root.join("usr/share/vulkan/icd.d/radeon_icd.i686.json"),
            "{}",
        );
        assert_eq!(probe_vulkan32(&env).status, HostStatus::Ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_32bit_ignores_a_64bit_library() {
        let root = temp_dir("vulkan32-lib64");
        let env = base_env(&root);
        write(&root.join("usr/lib32/libvulkan.so.1"), "");
        write(&root.join("usr/lib/libGLX_nvidia.so.0"), "");
        write(
            &root.join("usr/share/vulkan/icd.d/nvidia_icd.json"),
            r#"{"ICD":{"library_path":"libGLX_nvidia.so.0"}}"#,
        );

        // The soname exists only in the 64-bit folder: the 32-bit loader cannot
        // open it, so the finding stays actionable.
        let check = probe_vulkan32(&env);
        assert_eq!(check.status, HostStatus::TooOld);
        assert!(check.hint.unwrap_or_default().contains("nvidia_icd.json"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_32bit_unreadable_manifests_are_unknown() {
        let root = temp_dir("vulkan32-unreadable");
        let env = base_env(&root);
        write(&root.join("usr/lib32/libvulkan.so.1"), "");
        write(
            &root.join("usr/share/vulkan/icd.d/nvidia_icd.json"),
            "not json at all",
        );

        let check = probe_vulkan32(&env);
        assert_eq!(check.status, HostStatus::Unknown, "{}", check.detail);
        assert!(check.detail.contains("none of the Vulkan ICD manifests"));

        // One parseable manifest is enough to classify the rest.
        write(
            &root.join("usr/share/vulkan/icd.d/intel_icd.x86_64.json"),
            "{}",
        );
        assert_eq!(probe_vulkan32(&env).status, HostStatus::TooOld);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vulkan_32bit_guidance_never_offers_a_driver_transaction() {
        let manual = vulkan32_manual();
        let text = format!("{} {}", manual.instruction, manual.url.unwrap_or_default())
            .to_ascii_lowercase();
        for forbidden in [
            "nvidia-dkms",
            "dkms",
            "pacman -r",
            "pacman -rn",
            "pacman -rns",
            "apt remove",
            "apt-get remove",
            "dnf remove",
            "zypper remove",
            "uninstall",
            "remove nvidia",
        ] {
            assert!(
                !text.contains(forbidden),
                "the 32-bit guidance must not suggest {forbidden}"
            );
        }
        for expected in ["vulkaninfo --summary", "ldd", "pacman -qo", "dpkg -s"] {
            assert!(
                text.contains(expected),
                "the 32-bit guidance should mention {expected}"
            );
        }
    }

    #[test]
    fn umu_and_proton_come_from_the_compat_scan() {
        let root = temp_dir("compat");
        let mut env = base_env(&root);
        env.compat = Some(compat_with_umu_and_proton());

        assert_eq!(probe_umu(&env).status, HostStatus::Ok);
        let proton = probe_proton(&env);
        assert_eq!(proton.status, HostStatus::Ok);
        assert!(proton.detail.contains("GE-Proton9-20"));

        env.compat = Some(empty_compat());
        assert_eq!(probe_umu(&env).status, HostStatus::Missing);
        assert_eq!(probe_proton(&env).status, HostStatus::Missing);

        env.compat = None;
        assert_eq!(probe_umu(&env).status, HostStatus::Unknown);
        assert_eq!(probe_proton(&env).status, HostStatus::Unknown);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn compat_scan_fixture_trees_drive_the_proton_finding() {
        let root = temp_dir("compat-fixture");
        write(
            &root.join("steam/compatibilitytools.d/GE-Proton9-20/proton"),
            "",
        );
        let scan = compat::scan_with(&ScanPaths {
            home: Some(root.join("home")),
            steam_roots: vec![root.join("steam")],
            path_dirs: Vec::new(),
            display_root: Some(root.join("drm")),
            overrides: CompatOverrides::default(),
        });
        let mut env = base_env(&root);
        env.compat = Some(scan);
        assert_eq!(probe_proton(&env).status, HostStatus::Ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_tools_carry_the_family_packages() {
        let root = temp_dir("tools");
        let env = base_env(&root);
        let family = family_of(PackageFamily::Pacman, String::new());

        for (id, package) in [
            (FINDING_GAMEMODE, "gamemode"),
            (FINDING_MANGOHUD, "mangohud"),
            (FINDING_GAMESCOPE, "gamescope"),
        ] {
            let check = probe(&env, &family, id);
            assert_eq!(check.status, HostStatus::Missing);
            assert!(check.installable, "{id} should be installable");
            assert_eq!(check.packages, vec![package], "{id}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn installed_tools_are_ok_and_not_installable() {
        let root = temp_dir("tools-ok");
        let env = base_env(&root);
        write(&root.join("usr/bin/gamemoderun"), "");
        write(&root.join("usr/bin/gamescope"), "");
        write(&root.join("usr/lib/mangohud/libMangoHud.so"), "");
        let family = family_of(PackageFamily::Pacman, String::new());

        for id in [FINDING_GAMEMODE, FINDING_MANGOHUD, FINDING_GAMESCOPE] {
            let check = probe(&env, &family, id);
            assert_eq!(check.status, HostStatus::Ok, "{id}");
            assert!(!check.installable, "{id}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn webkit_41_ok_40_too_old_and_missing() {
        let root = temp_dir("webkit");
        let env = base_env(&root);
        assert_eq!(probe_webkit(&env).status, HostStatus::Missing);

        write(&root.join("usr/lib/libwebkit2gtk-4.0.so.0"), "");
        assert_eq!(probe_webkit(&env).status, HostStatus::TooOld);

        write(&root.join("usr/lib/libwebkit2gtk-4.1.so.0"), "");
        assert_eq!(probe_webkit(&env).status, HostStatus::Ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn family_detection_reads_os_release_then_binaries() {
        let root = temp_dir("family");
        let env = base_env(&root);
        for (id, expected) in [
            ("cachyos", PackageFamily::Pacman),
            ("fedora", PackageFamily::Dnf),
            ("ubuntu", PackageFamily::Apt),
            ("opensuse-tumbleweed", PackageFamily::Zypper),
        ] {
            os_release(&root, &format!("ID={id}\n"));
            assert_eq!(detect_family(&env).id, expected, "{id}");
        }

        os_release(&root, "ID=debian\nID_LIKE=\"ubuntu debian\"\n");
        assert_eq!(detect_family(&env).id, PackageFamily::Apt);

        os_release(&root, "ID=my-custom-os\n");
        assert_eq!(detect_family(&env).id, PackageFamily::Unknown);
        assert!(!detect_family(&env).installable);

        write(&root.join("usr/bin/apt"), "");
        assert_eq!(
            detect_family(&env).id,
            PackageFamily::Apt,
            "an unknown ID falls back to the installed package manager"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recipe_table_names_one_package_per_family_and_manual_where_absent() {
        for recipe in RECIPES {
            for family in [
                PackageFamily::Pacman,
                PackageFamily::Dnf,
                PackageFamily::Apt,
                PackageFamily::Zypper,
            ] {
                let packages = family.packages(recipe);
                match (recipe.finding, family) {
                    (FINDING_UMU, PackageFamily::Apt) => {
                        assert!(packages.is_none(), "UMU has no apt package");
                    }
                    _ => assert!(
                        packages.is_some_and(|list| !list.is_empty()),
                        "{}/{} should have a package",
                        recipe.finding,
                        family.as_str()
                    ),
                }
            }
        }
        assert!(recipe_for(FINDING_GPU).is_none(), "drivers have no recipe");
        assert!(
            recipe_for(FINDING_PROTON).is_none(),
            "Proton is not a package"
        );
        assert_eq!(
            PackageFamily::Apt.packages(recipe_for(FINDING_UMU).unwrap()),
            None
        );
        assert_eq!(
            PackageFamily::Apt.packages(recipe_for(FINDING_WEBKIT).unwrap()),
            Some(&["libwebkit2gtk-4.1-0"][..])
        );
    }

    #[test]
    fn install_command_plan_is_per_family() {
        assert_eq!(
            install_argv(PackageFamily::Pacman, &["umu-launcher".to_string()]).unwrap(),
            vec![
                "pkexec",
                "pacman",
                "-S",
                "--needed",
                "--noconfirm",
                "umu-launcher"
            ]
        );
        assert_eq!(
            install_argv(PackageFamily::Dnf, &["gamemode".to_string()]).unwrap(),
            vec!["pkexec", "dnf", "install", "-y", "gamemode"]
        );
        assert_eq!(
            install_argv(PackageFamily::Apt, &["mangohud".to_string()]).unwrap(),
            vec!["pkexec", "apt-get", "install", "-y", "mangohud"]
        );
        assert_eq!(
            install_argv(PackageFamily::Zypper, &["gamescope".to_string()]).unwrap(),
            vec![
                "pkexec",
                "zypper",
                "--non-interactive",
                "install",
                "gamescope"
            ]
        );
        assert!(install_argv(PackageFamily::Unknown, &["x".to_string()]).is_none());
        assert!(install_argv(PackageFamily::Pacman, &[]).is_none());
    }

    #[test]
    fn plan_installs_the_selected_recipes_and_refuses_everything_else() {
        let root = temp_dir("plan");
        let env = base_env(&root);
        os_release(&root, "ID=fedora\n");
        let report = detect_with(&env);

        let plan = plan_install(
            &report,
            &[
                FINDING_GAMEMODE.to_string(),
                FINDING_MANGOHUD.to_string(),
                FINDING_GPU.to_string(),
                "not-a-check".to_string(),
                FINDING_GAMEMODE.to_string(),
            ],
        );
        assert_eq!(plan.family.id, PackageFamily::Dnf);
        assert_eq!(
            plan.command,
            vec!["pkexec", "dnf", "install", "-y", "gamemode", "mangohud"]
        );
        assert_eq!(
            plan.command_label,
            "pkexec dnf install -y gamemode mangohud"
        );
        assert_eq!(plan.packages.len(), 2, "duplicates are collapsed");
        assert_eq!(plan.refused.len(), 2);
        assert!(plan
            .refused
            .iter()
            .any(|refusal| refusal.finding == FINDING_GPU));
        assert!(plan
            .refused
            .iter()
            .any(|refusal| refusal.finding == "not-a-check"));

        // Dry run never executes anything: the plan is returned as data.
        assert!(install_guard(true, false).is_ok());
        assert!(install_guard(false, false).is_err());
        assert!(install_guard(false, true).is_ok());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unknown_distribution_builds_no_command_and_offers_manual_steps() {
        let root = temp_dir("unknown");
        os_release(&root, "ID=my-custom-os\n");
        let env = base_env(&root);
        let report = detect_with(&env);
        assert_eq!(report.family.id, PackageFamily::Unknown);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("package manager")));

        let plan = plan_install(&report, &[FINDING_GAMEMODE.to_string()]);
        assert!(plan.command.is_empty());
        assert!(plan.packages.is_empty());
        // The finding itself still carries the manual instruction.
        assert!(finding(&report, FINDING_GAMEMODE).manual.is_some());
        assert!(install_argv(report.family.id, &["gamemode".to_string()]).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_recipe_can_ever_install_a_driver() {
        let forbidden = ["nvidia", "nouveau", "mesa", "amdgpu", "i915", "firmware"];
        for recipe in RECIPES {
            for package in [recipe.pacman, recipe.dnf, recipe.apt, recipe.zypper].concat() {
                let lowered = package.to_ascii_lowercase();
                for needle in forbidden {
                    assert!(
                        !lowered.contains(needle),
                        "{} must never be installed by Ludex",
                        package
                    );
                }
            }
        }
        assert!(RECIPES.iter().all(|recipe| recipe.finding != FINDING_GPU));
    }

    #[test]
    fn everything_outside_linux_is_unknown_and_never_installable() {
        let root = temp_dir("unsupported");
        let mut env = base_env(&root);
        env.supported = false;
        let report = detect_with(&env);
        assert!(!report.supported);
        assert!(report
            .findings
            .iter()
            .all(|finding| finding.status == HostStatus::Unknown));
        assert!(report.findings.iter().all(|finding| !finding.installable));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn persisted_report_round_trips_through_settings() {
        let root = temp_dir("persist");
        os_release(&root, "ID=cachyos\n");
        let env = base_env(&root);
        let report = detect_with(&env);

        let mut settings = LocalSettings::for_app_data(&root);
        settings.host_preflight = Some(report.clone());
        let path = root.join("settings.json");
        crate::settings::save(&path, &settings).expect("save");
        let loaded = crate::settings::load(&path, &root);
        assert_eq!(loaded.host_preflight, Some(report));

        // An older settings file without the field stays valid.
        let legacy = root.join("legacy.json");
        std::fs::write(&legacy, "{\"mode\":\"local\"}").unwrap();
        assert!(crate::settings::load(&legacy, &root)
            .host_preflight
            .is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_full_report_is_ready_only_without_missing_or_old_pieces() {
        let root = temp_dir("ready");
        let mut env = base_env(&root);
        card(&root, "card0", "amdgpu", "1002");
        write(
            &root.join("usr/share/vulkan/icd.d/radeon_icd.x86_64.json"),
            "{}",
        );
        write(
            &root.join("usr/share/vulkan/icd.d/radeon_icd.i686.json"),
            "{}",
        );
        write(&root.join("usr/lib32/libvulkan.so.1"), "");
        write(&root.join("usr/lib/libwebkit2gtk-4.1.so.0"), "");
        write(&root.join("usr/bin/gamemoderun"), "");
        write(&root.join("usr/bin/gamescope"), "");
        write(&root.join("usr/lib/mangohud/libMangoHud.so"), "");
        env.compat = Some(compat_with_umu_and_proton());
        os_release(&root, "ID=arch\n");

        let report = detect_with(&env);
        assert!(report.ready, "{:?}", report.findings);
        assert_eq!(report.family.id, PackageFamily::Pacman);
        assert_eq!(report.findings.len(), FINDING_ORDER.len());

        // One gap is enough to lose readiness.
        std::fs::remove_file(root.join("usr/bin/gamescope")).unwrap();
        assert!(!detect_with(&env).ready);
        std::fs::remove_dir_all(&root).ok();
    }
}
