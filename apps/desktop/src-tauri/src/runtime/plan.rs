//! Diagnosis, numbered plan and wizard state machine for "Make it run"
//! (#178).
//!
//! [`diagnose`] is pure with respect to the host: it reads [`Probe`] and a game
//! context, never the network, and never downloads anything. The plan it
//! returns is exactly what `make_it_run_apply` may execute, so the UI can show
//! every URL, size, licence and command before the consent click.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::detect::{self, GpuSupport, HostInfo, Probe, RuntimeDetection};
use super::{
    Platform, RuntimeAuditEvent, RuntimeCatalog, RuntimeInstallRow, RuntimeProvider,
    DISK_RESERVE_BYTES,
};
use crate::linux_gaming::{self, LaunchMode, LaunchOptions};

/// Per-game context the plan is built against.
#[derive(Debug, Clone)]
pub struct GameRuntimeContext {
    pub game_id: i64,
    pub title: String,
    pub install_path: PathBuf,
    pub executable: Option<String>,
    /// `x86`, `x86_64` or `aarch64`, read from the PE header when available.
    pub exe_machine: Option<String>,
    pub launch_options: LaunchOptions,
    /// Prerequisites the game itself declares (redistributables found in its
    /// folder, plus any manual override).
    pub prerequisites: Vec<String>,
    /// Existing tools found by [`crate::compat::scan`], so a Steam Proton or a
    /// system UMU already on the machine satisfies the plan (#178).
    pub compat_umu: Option<String>,
    pub compat_proton: Option<String>,
    pub compat_wine: Option<String>,
}

/// Stable prerequisite ids understood by the planner.
pub const PREREQUISITE_IDS: &[&str] = &["vcrun", "dotnet", "directx", "physx"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Info,
    Warning,
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIssue {
    /// Stable code the UI translates (`missing-umu`, `no-wrapper`, …).
    pub code: String,
    pub severity: IssueSeverity,
    pub title: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepAction {
    Install,
    Import,
    PreparePrefix,
    Configure,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Pending,
    Blocked,
    Running,
    Done,
    Failed,
    Skipped,
    NeedsElevation,
}

impl StepStatus {
    pub fn is_open(self) -> bool {
        matches!(
            self,
            StepStatus::Pending | StepStatus::Running | StepStatus::NeedsElevation
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDownload {
    pub url: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub licence: String,
    pub host: String,
}

/// One numbered step in the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStep {
    /// Stable id used by `make_it_run_apply` (`linux-umu`, `macos-gptk`, …).
    pub id: String,
    pub provider_id: Option<String>,
    pub action: StepAction,
    pub title: String,
    pub detail: String,
    pub requires_admin: bool,
    pub requires_consent: bool,
    pub download: Option<StepDownload>,
    pub manual: Option<super::ManualStep>,
    pub status: StepStatus,
    pub blocked_reason: Option<String>,
    /// Optional steps do not keep the plan from being ready (GPTK import).
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedRuntime {
    pub provider_id: String,
    pub name: String,
    pub version: String,
    pub path: String,
    pub size_bytes: u64,
    /// False when the recorded folder/file is gone.
    pub present: bool,
    pub imported: bool,
}

/// The complete diagnosis returned by `make_it_run_diagnose`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnosis {
    pub game_id: i64,
    pub title: String,
    pub platform: Platform,
    pub arch: String,
    pub os_version: Option<String>,
    pub elevated: bool,
    pub windows_game: bool,
    /// True when every required step is done and no blocking issue remains.
    pub ready: bool,
    /// True while a `make_it_run_apply` run holds this game.
    pub running: bool,
    pub summary: String,
    pub issues: Vec<RuntimeIssue>,
    pub steps: Vec<RuntimeStep>,
    pub detections: Vec<RuntimeDetection>,
    pub prerequisites: Vec<String>,
    pub gpu: GpuSupport,
    pub cache_dir: String,
    pub free_bytes: Option<u64>,
    pub required_bytes: u64,
    pub managed: Vec<ManagedRuntime>,
    pub audit: Vec<RuntimeAuditEvent>,
    pub notices: Vec<String>,
    pub generated_at: String,
}

/// Build issues, steps and provider detections for one game.
pub fn diagnose(
    catalog: &RuntimeCatalog,
    host: &HostInfo,
    probe: &dyn Probe,
    game: &GameRuntimeContext,
    records: &[RuntimeInstallRow],
    audit: &[RuntimeAuditEvent],
    now: &str,
) -> RuntimeDiagnosis {
    let mut detections = detect::detect_all(catalog, host, probe, records);
    apply_compat_tools(&mut detections, game);
    let gpu = detect::gpu_support(host, probe);
    let mut issues = Vec::new();
    let mut steps = Vec::new();

    let windows_game = host.platform == Platform::Linux && is_windows_game(game);
    match host.platform {
        Platform::Linux => plan_linux(
            catalog,
            host,
            probe,
            game,
            &detections,
            &gpu,
            &mut issues,
            &mut steps,
            windows_game,
        ),
        Platform::Windows => {
            plan_windows(catalog, host, game, &detections, &mut issues, &mut steps)
        }
        Platform::Macos => plan_macos(catalog, host, game, &detections, &mut issues, &mut steps),
    }

    // One disk check for every managed download in the plan. Archives need
    // roughly three times the compressed size while they are extracted.
    let required_bytes = step_download_bytes(&steps);
    if required_bytes > 0 {
        if let Some(free) = host.free_bytes {
            let needed = required_bytes
                .saturating_mul(3)
                .saturating_add(DISK_RESERVE_BYTES);
            if free < needed {
                issues.push(RuntimeIssue {
                    code: "disk-space".to_string(),
                    severity: IssueSeverity::Blocking,
                    title: "Not enough free disk space".to_string(),
                    detail: format!(
                        "The plan needs about {needed} bytes (download plus extraction) in {}, but only {free} bytes are free.",
                        host.cache_dir.display()
                    ),
                });
                for step in &mut steps {
                    if step.download.is_some() && step.status == StepStatus::Pending {
                        step.status = StepStatus::Blocked;
                        step.blocked_reason =
                            Some("Free disk space is too low for this download.".to_string());
                    }
                }
            }
        }
    }

    let blocking = issues
        .iter()
        .any(|issue| issue.severity == IssueSeverity::Blocking);
    let ready = !blocking
        && steps.iter().all(|step| {
            matches!(step.status, StepStatus::Done | StepStatus::Skipped)
                || (step.optional && step.status != StepStatus::Failed)
        });

    let managed = managed_runtimes(catalog, records, probe);
    let summary = if ready {
        "Everything this game needs is installed.".to_string()
    } else {
        let open = steps
            .iter()
            .filter(|step| step.status.is_open() && !step.optional)
            .count();
        let manual = steps
            .iter()
            .filter(|step| step.action == StepAction::Manual && step.status.is_open())
            .count();
        if manual > 0 {
            format!("{open} step(s) remain, {manual} of which only you can perform.")
        } else {
            format!("{open} step(s) remain before the runtime is ready.")
        }
    };

    RuntimeDiagnosis {
        game_id: game.game_id,
        title: game.title.clone(),
        platform: host.platform,
        arch: host.arch.clone(),
        os_version: host.os_version.clone(),
        elevated: host.elevated,
        windows_game,
        ready,
        running: false,
        summary,
        issues,
        steps,
        detections,
        prerequisites: game.prerequisites.clone(),
        gpu,
        cache_dir: host.cache_dir.to_string_lossy().to_string(),
        free_bytes: host.free_bytes,
        required_bytes,
        managed,
        audit: audit.to_vec(),
        notices: catalog.notices.clone(),
        generated_at: now.to_string(),
    }
}

fn apply_compat_tools(detections: &mut [RuntimeDetection], game: &GameRuntimeContext) {
    let overrides = [
        ("umu-launcher", game.compat_umu.as_deref(), "umu-run"),
        ("ge-proton", game.compat_proton.as_deref(), "proton"),
        ("wine-staging", game.compat_wine.as_deref(), "wine"),
    ];
    for (provider_id, path, label) in overrides {
        let Some(path) = path else {
            continue;
        };
        if let Some(detection) = detections
            .iter_mut()
            .find(|detection| detection.provider_id == provider_id)
        {
            if !detection.installed {
                detection.installed = true;
                detection.path = Some(path.to_string());
                detection.evidence = format!("found by compat_scan ({label})");
            }
        }
    }
}

fn is_windows_game(game: &GameRuntimeContext) -> bool {
    game.exe_machine.is_some()
        || game
            .executable
            .as_deref()
            .map(|executable| executable.to_ascii_lowercase().ends_with(".exe"))
            .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn plan_linux(
    catalog: &RuntimeCatalog,
    host: &HostInfo,
    probe: &dyn Probe,
    game: &GameRuntimeContext,
    detections: &[RuntimeDetection],
    gpu: &GpuSupport,
    issues: &mut Vec<RuntimeIssue>,
    steps: &mut Vec<RuntimeStep>,
    windows_game: bool,
) {
    if !windows_game {
        issues.push(RuntimeIssue {
            code: "native-game".to_string(),
            severity: IssueSeverity::Info,
            title: "Native game".to_string(),
            detail: "This game is not a Windows executable, so no compatibility runtime is needed."
                .to_string(),
        });
        return;
    }

    if host.arch != "x86_64" {
        issues.push(RuntimeIssue {
            code: "unsupported-arch".to_string(),
            severity: IssueSeverity::Blocking,
            title: "Unsupported CPU architecture".to_string(),
            detail: format!(
                "Proton builds are x86_64 only; this machine is {}. Use an x86_64 Linux host or the game's native build.",
                host.arch
            ),
        });
        steps.push(manual_step(
            "linux-arch",
            None,
            "Use an x86_64 Linux machine",
            "Ludex cannot install an x86_64 Proton runtime on this architecture.",
            super::ManualStep {
                label: "Supported platforms".to_string(),
                detail: "UMU and Proton run Windows games on x86_64 Linux.".to_string(),
                url: Some("https://github.com/Open-Wine-Components/umu-launcher".to_string()),
                command: None,
            },
        ));
        return;
    }

    let umu = detection(detections, "umu-launcher");
    let proton = detection(detections, "ge-proton");

    if !umu.map(|detection| detection.installed).unwrap_or(false) {
        issues.push(missing_issue(
            "missing-umu",
            "UMU Launcher is missing",
            "UMU runs the game through the Steam Linux Runtime and the pinned Proton build.",
        ));
        if let Some(provider) = catalog_provider(catalog, "umu-launcher") {
            steps.push(install_step(
                "linux-umu",
                provider,
                "Install UMU Launcher",
                "Downloads the pinned UMU zipapp and verifies its SHA-256 before use.",
            ));
        }
    }

    if !proton.map(|detection| detection.installed).unwrap_or(false) {
        issues.push(missing_issue(
            "missing-proton",
            "No Proton build is installed",
            "Ludex pins GE-Proton so every launch uses the same tested build.",
        ));
        if let Some(provider) = catalog_provider(catalog, "ge-proton") {
            steps.push(install_step(
                "linux-proton",
                provider,
                "Install the pinned Proton build",
                "Downloads GE-Proton, verifies its SHA-256, and keeps it in the Ludex runtime folder.",
            ));
        }
    }

    match linux_gaming::prefix_for(&game.install_path, &game.launch_options.prefix_path) {
        Err(error) => {
            issues.push(RuntimeIssue {
                code: "prefix-unsafe".to_string(),
                severity: IssueSeverity::Blocking,
                title: "The Wine prefix path is unsafe".to_string(),
                detail: error,
            });
            steps.push(configure_step(
                "linux-prefix-reset",
                "Reset the Wine prefix path",
                "The current prefix path would touch the install folder or escape it. Reset it to the per-game default.",
            ));
        }
        Ok(prefix) => {
            if prefix == game.install_path {
                issues.push(RuntimeIssue {
                    code: "prefix-unsafe".to_string(),
                    severity: IssueSeverity::Blocking,
                    title: "The Wine prefix is the install folder".to_string(),
                    detail: "A prefix must be a dedicated folder so resetting it never deletes the game."
                        .to_string(),
                });
                steps.push(configure_step(
                    "linux-prefix-reset",
                    "Reset the Wine prefix path",
                    "Move the prefix back to {installPath}/.prefix.",
                ));
            } else if !probe.dir_exists(&prefix) {
                issues.push(RuntimeIssue {
                    code: "prefix-missing".to_string(),
                    severity: IssueSeverity::Info,
                    title: "The per-game prefix does not exist yet".to_string(),
                    detail: format!(
                        "Ludex will create {} and record which runtime created it.",
                        prefix.display()
                    ),
                });
                steps.push(RuntimeStep {
                    id: "linux-prefix".to_string(),
                    provider_id: None,
                    action: StepAction::PreparePrefix,
                    title: "Create the per-game Wine prefix".to_string(),
                    detail: format!("Creates {} with a small marker file.", prefix.display()),
                    requires_admin: false,
                    requires_consent: false,
                    download: None,
                    manual: None,
                    status: StepStatus::Pending,
                    blocked_reason: None,
                    optional: false,
                });
            }
        }
    }

    if gpu.vulkan == Some(false) {
        issues.push(RuntimeIssue {
            code: "no-vulkan".to_string(),
            severity: IssueSeverity::Warning,
            title: "No Vulkan ICD found".to_string(),
            detail: gpu.note.clone(),
        });
    }

    if matches!(
        game.launch_options.mode,
        LaunchMode::Auto | LaunchMode::Native
    ) {
        issues.push(RuntimeIssue {
            code: "launch-mode".to_string(),
            severity: IssueSeverity::Info,
            title: "This game does not use UMU yet".to_string(),
            detail: "Ludex can point this game at UMU and its managed Proton build without changing other games."
                .to_string(),
        });
        steps.push(configure_step(
            "linux-configure",
            "Use the managed runtime for this game",
            "Sets the launch mode to UMU, clears a stale tool id and keeps the per-game prefix.",
        ));
    }
}

fn plan_windows(
    catalog: &RuntimeCatalog,
    host: &HostInfo,
    game: &GameRuntimeContext,
    detections: &[RuntimeDetection],
    issues: &mut Vec<RuntimeIssue>,
    steps: &mut Vec<RuntimeStep>,
) {
    let prerequisites: BTreeSet<&str> = game
        .prerequisites
        .iter()
        .map(|value| value.as_str())
        .collect();

    if host.arch == "x86" {
        issues.push(RuntimeIssue {
            code: "unsupported-arch".to_string(),
            severity: IssueSeverity::Blocking,
            title: "Unsupported Windows architecture".to_string(),
            detail: "Ludex ships x64 runtime installers; a 32-bit Windows host cannot run them."
                .to_string(),
        });
        return;
    }
    if host.arch == "aarch64" {
        issues.push(RuntimeIssue {
            code: "windows-arm".to_string(),
            severity: IssueSeverity::Warning,
            title: "Windows on Arm".to_string(),
            detail: "x64 redistributables run under Windows' emulation layer; a game that needs a native Arm build is out of scope."
                .to_string(),
        });
    }

    let vcrun_id = if game.exe_machine.as_deref() == Some("x86") {
        "vcrun-2022-x86"
    } else {
        "vcrun-2022-x64"
    };
    if !installed(detections, vcrun_id) {
        issues.push(missing_issue(
            "missing-vcrun",
            "The Visual C++ runtime is missing",
            "Most Windows games need the Microsoft Visual C++ 2015–2022 redistributable.",
        ));
        if let Some(provider) = catalog_provider(catalog, vcrun_id) {
            steps.push(install_step(
                vcrun_id,
                provider,
                "Install the Visual C++ runtime",
                "Runs Microsoft's signed redistributable with /install /quiet /norestart after verification.",
            ));
        }
    }

    if prerequisites.contains("dotnet") && !installed(detections, "dotnet-desktop-10") {
        issues.push(missing_issue(
            "missing-dotnet",
            ".NET Desktop Runtime is missing",
            "The game ships a .NET build; the desktop runtime is required before it can start.",
        ));
        if let Some(provider) = catalog_provider(catalog, "dotnet-desktop-10") {
            steps.push(install_step(
                "windows-dotnet",
                provider,
                "Install .NET Desktop Runtime 10",
                "Runs Microsoft's signed installer with /install /quiet /norestart after verification.",
            ));
        }
    }

    if prerequisites.contains("directx") && !installed(detections, "directx-jun2010") {
        issues.push(missing_issue(
            "missing-directx",
            "The DirectX runtime is missing",
            "The game folder contains a June 2010 DirectX redistributable.",
        ));
        if let Some(provider) = catalog_provider(catalog, "directx-jun2010") {
            steps.push(install_step(
                "windows-directx",
                provider,
                "Install the DirectX End-User Runtime",
                "Extracts Microsoft's redistributable, then runs DXSETUP.exe /silent. It is idempotent.",
            ));
        }
    }

    if prerequisites.contains("physx") && !installed(detections, "physx") {
        issues.push(missing_issue(
            "missing-physx",
            "PhysX is missing",
            "The game declares PhysX. Ludex links to NVIDIA instead of redistributing it.",
        ));
        if let Some(provider) = catalog_provider(catalog, "physx") {
            steps.push(manual_step(
                "windows-physx",
                Some(provider),
                "Install PhysX from NVIDIA",
                "Only NVIDIA distributes PhysX; Ludex never bundles it.",
                provider
                    .manual_steps
                    .first()
                    .cloned()
                    .unwrap_or(super::ManualStep {
                        label: "Open the NVIDIA PhysX page".to_string(),
                        detail: "Download and run NVIDIA's installer.".to_string(),
                        url: Some(provider.homepage.clone()),
                        command: None,
                    }),
            ));
        }
    }

    let needs_admin = steps.iter().any(|step| step.requires_admin);
    if needs_admin && !host.elevated {
        issues.push(RuntimeIssue {
            code: "admin-required".to_string(),
            severity: IssueSeverity::Info,
            title: "Administrator rights are needed".to_string(),
            detail: "Machine-wide redistributables need elevation. Ludex downloads and verifies the signed installer, then shows the exact command to run if the app is not elevated."
                .to_string(),
        });
    }
}

fn plan_macos(
    catalog: &RuntimeCatalog,
    host: &HostInfo,
    game: &GameRuntimeContext,
    detections: &[RuntimeDetection],
    issues: &mut Vec<RuntimeIssue>,
    steps: &mut Vec<RuntimeStep>,
) {
    let _ = game;
    let apple_silicon = host.arch == "aarch64";
    let wrappers = ["whisky", "heroic", "crossover", "sikarugir"];
    let installed_wrapper = wrappers
        .iter()
        .find(|id| installed(detections, id))
        .copied();

    if apple_silicon && !installed(detections, "rosetta-2") {
        issues.push(missing_issue(
            "rosetta-missing",
            "Rosetta 2 is not installed",
            "Intel-only Windows code needs Rosetta 2 on Apple Silicon. macOS can also prompt for it on first use.",
        ));
        if let Some(provider) = catalog_provider(catalog, "rosetta-2") {
            steps.push(manual_step(
                "macos-rosetta",
                Some(provider),
                "Install Rosetta 2",
                "Run the displayed command in Terminal; macOS asks for your password.",
                provider
                    .manual_steps
                    .first()
                    .cloned()
                    .unwrap_or(super::ManualStep {
                        label: "Install Rosetta 2".to_string(),
                        detail: "Run softwareupdate --install-rosetta --agree-to-license."
                            .to_string(),
                        url: None,
                        command: Some(
                            "softwareupdate --install-rosetta --agree-to-license".to_string(),
                        ),
                    }),
            ));
        }
    }

    if installed_wrapper.is_none() {
        issues.push(RuntimeIssue {
            code: "no-wrapper".to_string(),
            severity: IssueSeverity::Blocking,
            title: "No macOS compatibility wrapper found".to_string(),
            detail: if apple_silicon {
                "Install Whisky, Heroic or CrossOver, or select a wrapper you already have. Ludex never bundles them."
                    .to_string()
            } else {
                "On Intel Macs, CrossOver 26 is the last release that supports this hardware."
                    .to_string()
            },
        });
        let candidates: &[(&str, &str, &str)] = if apple_silicon {
            &[
                (
                    "macos-whisky",
                    "whisky",
                    "Install Whisky, a signed Wine 11 wrapper",
                ),
                (
                    "macos-heroic",
                    "heroic",
                    "Install Heroic, which manages its own Wine builds",
                ),
                ("macos-crossover", "crossover", "Buy and install CrossOver"),
            ]
        } else {
            &[
                (
                    "macos-crossover",
                    "crossover",
                    "Install CrossOver 26 (last Intel release)",
                ),
                (
                    "macos-heroic",
                    "heroic",
                    "Install Heroic, which manages its own Wine builds",
                ),
                ("macos-sikarugir", "sikarugir", "Install Sikarugir"),
            ]
        };
        for (step_id, provider_id, title) in candidates {
            if let Some(provider) = catalog_provider(catalog, provider_id) {
                steps.push(manual_step(
                    step_id,
                    Some(provider),
                    title,
                    "Ludex opens the official page; the download and licence are yours to accept.",
                    provider
                        .manual_steps
                        .first()
                        .cloned()
                        .unwrap_or(super::ManualStep {
                            label: title.to_string(),
                            detail: provider.summary.clone(),
                            url: Some(provider.homepage.clone()),
                            command: None,
                        }),
                ));
            }
        }
    }

    if !apple_silicon {
        issues.push(RuntimeIssue {
            code: "intel-end-of-life".to_string(),
            severity: IssueSeverity::Warning,
            title: "Intel Mac support is ending".to_string(),
            detail: "macOS 26/27/28 drop Intel hardware, and CrossOver 26 is the last Intel-capable release. Keep that risk in mind before buying a licence."
                .to_string(),
        });
    } else {
        if host.is_macos_15_or_newer() == Some(false) {
            issues.push(RuntimeIssue {
                code: "macos-too-old".to_string(),
                severity: IssueSeverity::Warning,
                title: "macOS 15 or newer is required by Whisky".to_string(),
                detail: "On an older macOS, use Heroic or CrossOver instead.".to_string(),
            });
        }

        if !installed(detections, "gptk") {
            issues.push(RuntimeIssue {
                code: "gptk-optional".to_string(),
                severity: IssueSeverity::Info,
                title: "Game Porting Toolkit is optional".to_string(),
                detail: "Download the DMG from Apple with your own Apple ID, then import it. Ludex verifies and records it, and never copies or bundles it."
                    .to_string(),
            });
            if let Some(provider) = catalog_provider(catalog, "gptk") {
                let mut step = install_step(
                    "macos-gptk",
                    provider,
                    "Import a Game Porting Toolkit DMG",
                    "Verifies the file's size and SHA-256, shows the evaluation licence, and records its path. Nothing is copied.",
                );
                step.action = StepAction::Import;
                step.download = None;
                step.optional = true;
                steps.push(step);
            }
        }
    }
}

fn step_download_bytes(steps: &[RuntimeStep]) -> u64 {
    let mut seen = BTreeSet::new();
    steps
        .iter()
        .filter_map(|step| {
            let download = step.download.as_ref()?;
            seen.insert(download.url.clone())
                .then_some(download.size_bytes)
        })
        .sum()
}

fn managed_runtimes(
    catalog: &RuntimeCatalog,
    records: &[RuntimeInstallRow],
    probe: &dyn Probe,
) -> Vec<ManagedRuntime> {
    let mut result = Vec::new();
    for record in records {
        let Some(provider) = catalog
            .providers
            .iter()
            .find(|provider| provider.id == record.provider_id)
        else {
            continue;
        };
        if !provider.managed
            && provider
                .install
                .as_ref()
                .map(|install| install.kind != super::InstallKind::Import)
                .unwrap_or(true)
        {
            continue;
        }
        let present = if record.path.is_empty() {
            true
        } else {
            probe.dir_exists(Path::new(&record.path)) || probe.path_exists(Path::new(&record.path))
        };
        result.push(ManagedRuntime {
            provider_id: provider.id.clone(),
            name: provider.name.clone(),
            version: record.version.clone(),
            path: record.path.clone(),
            size_bytes: record.size_bytes,
            present,
            imported: provider
                .install
                .as_ref()
                .map(|install| install.kind == super::InstallKind::Import)
                .unwrap_or(false),
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    result
}

fn detection<'a>(detections: &'a [RuntimeDetection], id: &str) -> Option<&'a RuntimeDetection> {
    detections
        .iter()
        .find(|detection| detection.provider_id == id)
}

fn installed(detections: &[RuntimeDetection], id: &str) -> bool {
    detection(detections, id)
        .map(|detection| detection.installed)
        .unwrap_or(false)
}

fn catalog_provider<'a>(catalog: &'a RuntimeCatalog, id: &str) -> Option<&'a RuntimeProvider> {
    catalog.providers.iter().find(|provider| provider.id == id)
}

fn install_step(id: &str, provider: &RuntimeProvider, title: &str, detail: &str) -> RuntimeStep {
    let download = provider.download.as_ref().map(|download| StepDownload {
        url: download.url.clone(),
        size_bytes: download.size_bytes,
        sha256: download.sha256.clone(),
        licence: provider.licence.clone(),
        host: reqwest::Url::parse(&download.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_default(),
    });
    RuntimeStep {
        id: id.to_string(),
        provider_id: Some(provider.id.clone()),
        action: StepAction::Install,
        title: title.to_string(),
        detail: format!(
            "{detail} {} {} · {}",
            provider.name, provider.version, provider.licence
        ),
        requires_admin: provider.requires_admin,
        requires_consent: provider.requires_consent,
        download,
        manual: provider.manual_steps.first().cloned(),
        status: StepStatus::Pending,
        blocked_reason: None,
        optional: false,
    }
}

fn configure_step(id: &str, title: &str, detail: &str) -> RuntimeStep {
    RuntimeStep {
        id: id.to_string(),
        provider_id: None,
        action: StepAction::Configure,
        title: title.to_string(),
        detail: detail.to_string(),
        requires_admin: false,
        requires_consent: false,
        download: None,
        manual: None,
        status: StepStatus::Pending,
        blocked_reason: None,
        optional: false,
    }
}

fn manual_step(
    id: &str,
    provider: Option<&RuntimeProvider>,
    title: &str,
    detail: &str,
    manual: super::ManualStep,
) -> RuntimeStep {
    RuntimeStep {
        id: id.to_string(),
        provider_id: provider.map(|provider| provider.id.clone()),
        action: StepAction::Manual,
        title: title.to_string(),
        detail: detail.to_string(),
        requires_admin: provider
            .map(|provider| provider.requires_admin)
            .unwrap_or(false),
        requires_consent: false,
        download: None,
        manual: Some(manual),
        status: StepStatus::Pending,
        blocked_reason: None,
        optional: false,
    }
}

fn missing_issue(code: &str, title: &str, detail: &str) -> RuntimeIssue {
    RuntimeIssue {
        code: code.to_string(),
        severity: IssueSeverity::Warning,
        title: title.to_string(),
        detail: detail.to_string(),
    }
}

/// Find redistributable installers a game ships, without leaving its folder.
///
/// Bounded to three levels and 500 entries. Only names are inspected; nothing
/// is executed or opened. Values are stable [`PREREQUISITE_IDS`].
pub fn declared_prerequisites(install_path: &Path) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut queue = vec![(install_path.to_path_buf(), 0usize)];
    let mut entries = 0usize;
    while let Some((folder, depth)) = queue.pop() {
        if depth > 3 || entries > 500 {
            break;
        }
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in read.flatten() {
            entries += 1;
            if entries > 500 {
                break;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                queue.push((path, depth + 1));
                continue;
            }
            let name = entry
                .file_name()
                .to_str()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if name.contains("vcredist") || name.contains("vc_redist") {
                found.insert("vcrun".to_string());
            }
            if name.starts_with("dotnet") || name.starts_with("ndp") {
                found.insert("dotnet".to_string());
            }
            if name == "dxsetup.exe" || name.contains("directx") {
                found.insert("directx".to_string());
            }
            if name.contains("physx") {
                found.insert("physx".to_string());
            }
        }
    }
    found.into_iter().collect()
}

/// Read the PE machine of an executable. Returns `x86`, `x86_64`, `aarch64` or
/// `None` for a non-PE file.
pub fn pe_machine(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 4096];
    let read = file.read(&mut header).ok()?;
    if read < 0x40 || &header[0..2] != b"MZ" {
        return None;
    }
    let offset =
        u32::from_le_bytes([header[0x3c], header[0x3d], header[0x3e], header[0x3f]]) as usize;
    if offset + 6 > read || &header[offset..offset + 4] != b"PE\0\0" {
        return None;
    }
    let machine = u16::from_le_bytes([header[offset + 4], header[offset + 5]]);
    match machine {
        0x014c => Some("x86".to_string()),
        0x8664 => Some("x86_64".to_string()),
        0xaa64 => Some("aarch64".to_string()),
        _ => None,
    }
}

/// Wizard phases exposed to the UI. The backend owns the transitions; the web
/// view only renders them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WizardPhase {
    Idle,
    Planned,
    Installing,
    Verifying,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardEvent {
    Scan,
    Consent,
    StepStarted,
    StepDone,
    Verify,
    Succeed,
    Fail,
    Cancel,
    /// Reserved for the UI's "start over" button; tested here.
    #[allow(dead_code)]
    Reset,
}

/// Explicit state machine for one apply run. Illegal transitions are errors,
/// not silently ignored, so a stale UI cannot start an install out of order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WizardState {
    pub phase: WizardPhase,
    pub current_step: Option<String>,
    pub completed_steps: Vec<String>,
}

impl Default for WizardState {
    fn default() -> Self {
        Self {
            phase: WizardPhase::Idle,
            current_step: None,
            completed_steps: Vec::new(),
        }
    }
}

impl WizardState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: WizardEvent) -> Result<(), String> {
        use WizardEvent as E;
        use WizardPhase as P;
        let next = match (self.phase, event) {
            (P::Idle, E::Scan) => P::Planned,
            (P::Idle, E::Reset) => P::Idle,
            (P::Planned, E::Scan) => P::Planned,
            (P::Planned, E::Consent) => P::Installing,
            (P::Planned, E::Fail) => P::Failed,
            (P::Planned, E::Cancel) => P::Cancelled,
            (P::Installing, E::StepStarted) => P::Installing,
            (P::Installing, E::StepDone) => P::Installing,
            (P::Installing, E::Verify) => P::Verifying,
            (P::Installing, E::Fail) => P::Failed,
            (P::Installing, E::Cancel) => P::Cancelled,
            (P::Verifying, E::Succeed) => P::Complete,
            (P::Verifying, E::Fail) => P::Failed,
            (P::Verifying, E::Cancel) => P::Cancelled,
            (P::Complete, E::Reset) => P::Idle,
            (P::Failed, E::Reset) => P::Idle,
            (P::Failed, E::Consent) => P::Installing,
            (P::Cancelled, E::Reset) => P::Idle,
            (P::Cancelled, E::Consent) => P::Installing,
            (phase, event) => {
                return Err(format!(
                    "Illegal Make it run transition: {phase:?} cannot handle {event:?}."
                ))
            }
        };
        if event == E::StepStarted {
            // `current_step` is filled by the caller through [set_step].
        }
        self.phase = next;
        Ok(())
    }

    pub fn set_step(&mut self, step_id: Option<String>) {
        self.current_step = step_id;
    }

    pub fn complete_step(&mut self, step_id: &str) {
        if !self.completed_steps.iter().any(|done| done == step_id) {
            self.completed_steps.push(step_id.to_string());
        }
        self.current_step = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::detect::FixtureProbe;
    use crate::runtime::{catalog, RuntimeInstallRow};

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
            free_bytes: Some(100 * 1024 * 1024 * 1024),
        }
    }

    fn game(_platform: Platform) -> GameRuntimeContext {
        GameRuntimeContext {
            game_id: 7,
            title: "Test Game".to_string(),
            install_path: PathBuf::from("/games/test"),
            executable: Some("game.exe".to_string()),
            exe_machine: Some("x86_64".to_string()),
            launch_options: LaunchOptions::default(),
            prerequisites: Vec::new(),
            compat_umu: None,
            compat_proton: None,
            compat_wine: None,
        }
    }

    fn diagnose_with(
        platform: Platform,
        arch: &str,
        probe: &FixtureProbe,
        game: &GameRuntimeContext,
    ) -> RuntimeDiagnosis {
        diagnose(
            &catalog::build(),
            &host(platform, arch),
            probe,
            game,
            &[],
            &[],
            "2026-11-12T00:00:00Z",
        )
    }

    fn step<'a>(diagnosis: &'a RuntimeDiagnosis, id: &str) -> &'a RuntimeStep {
        diagnosis
            .steps
            .iter()
            .find(|step| step.id == id)
            .unwrap_or_else(|| panic!("missing step {id}: {:?}", diagnosis.steps))
    }

    #[test]
    fn a_native_linux_game_needs_nothing() {
        let mut context = game(Platform::Linux);
        context.executable = Some("game.sh".to_string());
        context.exe_machine = None;
        let diagnosis = diagnose_with(
            Platform::Linux,
            "x86_64",
            &FixtureProbe::default(),
            &context,
        );
        assert!(diagnosis.ready);
        assert!(diagnosis.steps.is_empty());
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "native-game"));
    }

    #[test]
    fn an_empty_linux_machine_gets_umu_proton_prefix_and_configure() {
        let diagnosis = diagnose_with(
            Platform::Linux,
            "x86_64",
            &FixtureProbe::default(),
            &game(Platform::Linux),
        );
        assert!(!diagnosis.ready);
        let ids: Vec<&str> = diagnosis
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![
                "linux-umu",
                "linux-proton",
                "linux-prefix",
                "linux-configure"
            ]
        );
        let umu = step(&diagnosis, "linux-umu");
        assert_eq!(umu.action, StepAction::Install);
        assert_eq!(umu.download.as_ref().unwrap().size_bytes, 430_080);
        assert_eq!(umu.download.as_ref().unwrap().host, "github.com");
        assert!(umu.requires_consent);
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "missing-umu"));
    }

    #[test]
    fn installed_umu_and_proton_only_need_a_prefix_and_configuration() {
        let probe = FixtureProbe::default()
            .file("/usr/bin/umu-run")
            .file("/home/tester/.local/share/ludex/runtimes/proton/GE-Proton11-12/proton");
        let records = vec![RuntimeInstallRow {
            provider_id: "ge-proton".to_string(),
            platform: "linux".to_string(),
            version: "11-12".to_string(),
            path: "/home/tester/.local/share/ludex/runtimes/proton/GE-Proton11-12".to_string(),
            sha256: String::new(),
            size_bytes: 0,
            installed_at: "2026-11-12T00:00:00Z".to_string(),
        }];
        let diagnosis = diagnose(
            &catalog::build(),
            &host(Platform::Linux, "x86_64"),
            &probe,
            &game(Platform::Linux),
            &records,
            &[],
            "2026-11-12T00:00:00Z",
        );
        let ids: Vec<&str> = diagnosis
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        assert_eq!(ids, vec!["linux-prefix", "linux-configure"]);
        assert!(
            diagnosis
                .detections
                .iter()
                .find(|detection| detection.provider_id == "ge-proton")
                .unwrap()
                .installed
        );
    }

    #[test]
    fn a_fully_configured_linux_game_is_ready_and_has_no_step() {
        let probe = FixtureProbe::default()
            .file("/usr/bin/umu-run")
            .file("/home/tester/.local/share/ludex/runtimes/proton/GE-Proton11-12/proton")
            .dir("/games/test/.prefix");
        let records = vec![RuntimeInstallRow {
            provider_id: "ge-proton".to_string(),
            platform: "linux".to_string(),
            version: "11-12".to_string(),
            path: "/home/tester/.local/share/ludex/runtimes/proton/GE-Proton11-12".to_string(),
            sha256: String::new(),
            size_bytes: 0,
            installed_at: "2026-11-12T00:00:00Z".to_string(),
        }];
        let mut context = game(Platform::Linux);
        context.launch_options.mode = LaunchMode::Umu;
        let diagnosis = diagnose(
            &catalog::build(),
            &host(Platform::Linux, "x86_64"),
            &probe,
            &context,
            &records,
            &[],
            "2026-11-12T00:00:00Z",
        );
        assert!(
            diagnosis.ready,
            "steps: {:?} issues: {:?}",
            diagnosis.steps, diagnosis.issues
        );
        assert!(diagnosis.steps.is_empty());
    }

    #[test]
    fn an_existing_steam_proton_and_umu_satisfy_the_plan() {
        let probe = FixtureProbe::default().dir("/games/test/.prefix");
        let mut context = game(Platform::Linux);
        context.launch_options.mode = LaunchMode::Umu;
        context.compat_umu = Some("/home/tester/.local/bin/umu-run".to_string());
        context.compat_proton =
            Some("/home/tester/.steam/steam/steamapps/common/Proton - Experimental".to_string());
        let diagnosis = diagnose_with(Platform::Linux, "x86_64", &probe, &context);
        assert!(
            diagnosis.ready,
            "steps: {:?} issues: {:?}",
            diagnosis.steps, diagnosis.issues
        );
        assert!(diagnosis.steps.is_empty());
        let proton = diagnosis
            .detections
            .iter()
            .find(|detection| detection.provider_id == "ge-proton")
            .unwrap();
        assert!(proton.installed);
        assert!(proton.evidence.contains("compat_scan"));
    }

    #[test]
    fn an_unsafe_prefix_is_blocking_and_gets_a_reset_step() {
        let probe = FixtureProbe::default()
            .file("/usr/bin/umu-run")
            .file("/usr/bin/proton")
            .dir("/games/test/.prefix");
        let mut context = game(Platform::Linux);
        context.launch_options.prefix_path = "../outside".to_string();
        let diagnosis = diagnose_with(Platform::Linux, "x86_64", &probe, &context);
        assert!(!diagnosis.ready);
        assert!(diagnosis.issues.iter().any(
            |issue| issue.code == "prefix-unsafe" && issue.severity == IssueSeverity::Blocking
        ));
        assert_eq!(
            step(&diagnosis, "linux-prefix-reset").action,
            StepAction::Configure
        );
    }

    #[test]
    fn arm_linux_is_blocked_before_any_download() {
        let diagnosis = diagnose_with(
            Platform::Linux,
            "aarch64",
            &FixtureProbe::default(),
            &game(Platform::Linux),
        );
        assert!(!diagnosis.ready);
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "unsupported-arch"));
        assert_eq!(diagnosis.steps.len(), 1);
        assert_eq!(diagnosis.steps[0].action, StepAction::Manual);
    }

    #[test]
    fn windows_missing_vcrun_and_declared_dotnet_get_install_steps() {
        let mut context = game(Platform::Windows);
        context.executable = Some("game.exe".to_string());
        context.prerequisites = vec!["dotnet".to_string(), "directx".to_string()];
        let diagnosis = diagnose_with(
            Platform::Windows,
            "x86_64",
            &FixtureProbe::default(),
            &context,
        );
        let ids: Vec<&str> = diagnosis
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["vcrun-2022-x64", "windows-dotnet", "windows-directx"]
        );
        assert!(diagnosis.steps.iter().all(|step| step.requires_admin));
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "admin-required"));
        assert!(!diagnosis.ready);
    }

    #[test]
    fn windows_with_vcrun_installed_is_a_noop_when_nothing_is_declared() {
        let key = "HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64";
        let probe = FixtureProbe::default()
            .registry(key, "Installed", "1")
            .registry(key, "Version", "14.44.35211");
        let mut context = game(Platform::Windows);
        context.executable = Some("game.exe".to_string());
        let diagnosis = diagnose_with(Platform::Windows, "x86_64", &probe, &context);
        assert!(diagnosis.ready, "issues: {:?}", diagnosis.issues);
        assert!(diagnosis.steps.is_empty());
    }

    #[test]
    fn a_32_bit_game_uses_the_x86_redistributable() {
        let mut context = game(Platform::Windows);
        context.exe_machine = Some("x86".to_string());
        let diagnosis = diagnose_with(
            Platform::Windows,
            "x86_64",
            &FixtureProbe::default(),
            &context,
        );
        assert_eq!(diagnosis.steps[0].id, "vcrun-2022-x86");
    }

    #[test]
    fn macos_without_a_wrapper_gets_manual_steps_and_an_optional_gptk_import() {
        let diagnosis = diagnose_with(
            Platform::Macos,
            "aarch64",
            &FixtureProbe::default(),
            &game(Platform::Macos),
        );
        assert!(!diagnosis.ready);
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "no-wrapper" && issue.severity == IssueSeverity::Blocking));
        assert!(diagnosis
            .steps
            .iter()
            .any(|step| step.id == "macos-whisky" && step.action == StepAction::Manual));
        let gptk = step(&diagnosis, "macos-gptk");
        assert_eq!(gptk.action, StepAction::Import);
        assert!(gptk.optional);
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "rosetta-missing"));
    }

    #[test]
    fn macos_with_crossover_is_ready_even_if_gptk_is_not_imported() {
        let probe = FixtureProbe::default()
            .dir("/Applications/CrossOver.app")
            .file("/Library/Apple/usr/libexec/oah/translate");
        let diagnosis = diagnose_with(Platform::Macos, "aarch64", &probe, &game(Platform::Macos));
        assert!(
            diagnosis.ready,
            "steps: {:?} issues: {:?}",
            diagnosis.steps, diagnosis.issues
        );
        assert_eq!(diagnosis.steps.len(), 1, "only the optional GPTK import");
        assert!(diagnosis.steps[0].optional);
    }

    #[test]
    fn imported_gptk_appears_in_the_managed_list() {
        let probe = FixtureProbe::default()
            .dir("/Applications/CrossOver.app")
            .file("/Library/Apple/usr/libexec/oah/translate")
            .file_with_size("/Users/tester/Downloads/gptk.dmg", 12);
        let records = vec![RuntimeInstallRow {
            provider_id: "gptk".to_string(),
            platform: "macos".to_string(),
            version: "user-supplied".to_string(),
            path: "/Users/tester/Downloads/gptk.dmg".to_string(),
            sha256: "abc".to_string(),
            size_bytes: 12,
            installed_at: "2026-11-12T00:00:00Z".to_string(),
        }];
        let diagnosis = diagnose(
            &catalog::build(),
            &host(Platform::Macos, "aarch64"),
            &probe,
            &game(Platform::Macos),
            &records,
            &[],
            "2026-11-12T00:00:00Z",
        );
        assert!(diagnosis.ready);
        assert!(diagnosis
            .managed
            .iter()
            .any(|runtime| runtime.provider_id == "gptk" && runtime.imported));
    }

    #[test]
    fn intel_macos_gets_the_end_of_life_warning_and_no_gptk() {
        let probe = FixtureProbe::default().dir("/Applications/Heroic.app");
        let diagnosis = diagnose_with(Platform::Macos, "x86_64", &probe, &game(Platform::Macos));
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "intel-end-of-life"));
        assert!(diagnosis.steps.iter().all(|step| step.id != "macos-gptk"));
    }

    #[test]
    fn low_disk_space_blocks_install_steps() {
        let mut low = host(Platform::Linux, "x86_64");
        low.free_bytes = Some(10 * 1024 * 1024);
        let diagnosis = diagnose(
            &catalog::build(),
            &low,
            &FixtureProbe::default(),
            &game(Platform::Linux),
            &[],
            &[],
            "2026-11-12T00:00:00Z",
        );
        assert!(!diagnosis.ready);
        assert!(diagnosis
            .issues
            .iter()
            .any(|issue| issue.code == "disk-space"));
        assert_eq!(step(&diagnosis, "linux-umu").status, StepStatus::Blocked);
    }

    #[test]
    fn declared_prerequisites_are_found_without_leaving_the_folder() {
        let root = std::env::temp_dir().join(format!(
            "ludex-runtime-prereq-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(root.join("_CommonRedist/vcredist")).unwrap();
        std::fs::write(root.join("_CommonRedist/vcredist/vcredist_x64.exe"), b"x").unwrap();
        std::fs::create_dir_all(root.join("dotnet")).unwrap();
        std::fs::write(root.join("dotnet/ndp48.exe"), b"x").unwrap();
        std::fs::write(root.join("DXSETUP.exe"), b"x").unwrap();
        let found = declared_prerequisites(&root);
        assert!(found.contains(&"vcrun".to_string()));
        assert!(found.contains(&"dotnet".to_string()));
        assert!(found.contains(&"directx".to_string()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pe_machine_reads_a_fake_executable() {
        let root = std::env::temp_dir().join(format!(
            "ludex-runtime-pe-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("game.exe");
        let mut bytes = vec![0u8; 512];
        bytes[0] = b'M';
        bytes[1] = b'Z';
        bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x84..0x86].copy_from_slice(&0x8664u16.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(pe_machine(&path), Some("x86_64".to_string()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn wizard_only_allows_expected_transitions() {
        let mut state = WizardState::new();
        assert!(state.apply(WizardEvent::Consent).is_err());
        state.apply(WizardEvent::Scan).unwrap();
        assert_eq!(state.phase, WizardPhase::Planned);
        state.apply(WizardEvent::Consent).unwrap();
        assert_eq!(state.phase, WizardPhase::Installing);
        state.set_step(Some("linux-umu".to_string()));
        state.apply(WizardEvent::StepStarted).unwrap();
        state.complete_step("linux-umu");
        state.apply(WizardEvent::StepDone).unwrap();
        state.set_step(None);
        state.apply(WizardEvent::Verify).unwrap();
        assert_eq!(state.phase, WizardPhase::Verifying);
        state.apply(WizardEvent::Succeed).unwrap();
        assert_eq!(state.phase, WizardPhase::Complete);
        assert_eq!(state.completed_steps, vec!["linux-umu".to_string()]);
        assert!(state.apply(WizardEvent::StepStarted).is_err());
        assert!(state.apply(WizardEvent::Scan).is_err());
        state.apply(WizardEvent::Reset).unwrap();
        assert_eq!(state.phase, WizardPhase::Idle);
        let mut state = WizardState::new();
        state.apply(WizardEvent::Scan).unwrap();
        state.apply(WizardEvent::Consent).unwrap();
        state.apply(WizardEvent::Cancel).unwrap();
        assert_eq!(state.phase, WizardPhase::Cancelled);
        state.apply(WizardEvent::Reset).unwrap();
        assert_eq!(state.phase, WizardPhase::Idle);
    }
}
