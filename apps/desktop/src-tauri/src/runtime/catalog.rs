//! The shipped runtime manifest and its download allow-list (#178).
//!
//! Versions, sizes and SHA-256 digests are pinned. The digests were read from
//! the signed release metadata at pin time (GitHub release assets expose a
//! `sha256:` digest; Microsoft's CDN does not publish one, so the digest of the
//! exact redistributable Ludex pins was recorded here). Changing any URL or
//! version without updating the digest makes the install fail closed.

use super::{
    ArchiveKind, DetectionSpec, DownloadArtifact, InstallKind, InstallSpec, ManifestRefresh,
    Platform, ProviderKind, RegistryCheck, RuntimeCatalog, RuntimeProvider, MAX_DOWNLOAD_BYTES,
};

/// Hosts a managed download may start from, or redirect to.
///
/// `github.com` release downloads redirect to `objects.githubusercontent.com`;
/// `aka.ms` and `dot.net` redirect to the Microsoft CDN hosts listed here.
pub const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "download.visualstudio.microsoft.com",
    "builds.dotnet.microsoft.com",
    "download.microsoft.com",
    "aka.ms",
];

/// Validate one catalog download URL. Pure so the allow-list is unit tested
/// without a network.
pub fn check_download_url(raw: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(raw).map_err(|error| format!("Invalid download URL: {error}"))?;
    if url.scheme() != "https" {
        return Err(format!(
            "Refusing a non-HTTPS runtime download: {}",
            url.scheme()
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Refusing a runtime download URL that carries credentials.".to_string());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "The runtime download URL has no host.".to_string())?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if !ALLOWED_HOSTS.iter().any(|allowed| *allowed == host) {
        return Err(format!(
            "Refusing a runtime download from an unknown host: {host}"
        ));
    }
    if !matches!(url.port(), None | Some(443)) {
        return Err("Refusing a runtime download on a non-standard port.".to_string());
    }
    if url.path().is_empty() || url.path() == "/" {
        return Err("The runtime download URL has no path.".to_string());
    }
    Ok(())
}

/// Structural validation used by tests and by `runtime_catalog` before the
/// manifest leaves the app.
pub fn validate(catalog: &RuntimeCatalog) -> Vec<String> {
    let mut problems = Vec::new();
    if catalog.schema_version == 0 {
        problems.push("schemaVersion must be positive".to_string());
    }
    for provider in &catalog.providers {
        let prefix = format!("{}: ", provider.id);
        if provider.managed {
            match &provider.download {
                None => problems.push(format!("{prefix}a managed provider needs a download")),
                Some(download) => {
                    if let Err(error) = check_download_url(&download.url) {
                        problems.push(format!("{prefix}{error}"));
                    }
                    if download.size_bytes == 0 || download.size_bytes > MAX_DOWNLOAD_BYTES {
                        problems.push(format!("{prefix}size is outside the allowed range"));
                    }
                    if download.sha256.len() != 64
                        || !download.sha256.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        problems.push(format!("{prefix}sha256 must be 64 hex characters"));
                    }
                    if download.archive == ArchiveKind::Dmg && provider.install.is_none() {
                        problems.push(format!("{prefix}a DMG provider needs an import spec"));
                    }
                }
            }
            if provider.install.is_none() {
                problems.push(format!("{prefix}a managed provider needs an install spec"));
            }
        } else if let Some(download) = &provider.download {
            if let Err(error) = check_download_url(&download.url) {
                problems.push(format!("{prefix}{error}"));
            }
        }
        for step in &provider.manual_steps {
            if let Some(url) = &step.url {
                let parsed = reqwest::Url::parse(url);
                if !matches!(parsed, Ok(parsed) if parsed.scheme() == "https") {
                    problems.push(format!("{prefix}manual links must be HTTPS"));
                }
            }
        }
    }
    problems
}

#[allow(clippy::too_many_arguments)]
fn provider(
    id: &str,
    platform: Platform,
    kind: ProviderKind,
    name: &str,
    version: &str,
    licence: &str,
    homepage: &str,
    summary: &str,
) -> RuntimeProvider {
    RuntimeProvider {
        id: id.to_string(),
        platform,
        kind,
        name: name.to_string(),
        version: version.to_string(),
        licence: licence.to_string(),
        homepage: homepage.to_string(),
        summary: summary.to_string(),
        managed: false,
        requires_admin: false,
        requires_consent: false,
        download: None,
        install: None,
        detection: DetectionSpec::default(),
        manual_steps: Vec::new(),
        notes: Vec::new(),
        arch: None,
    }
}

fn managed_download(
    url: &str,
    size_bytes: u64,
    sha256: &str,
    archive: ArchiveKind,
    strip_components: usize,
    entry_point: Option<&str>,
) -> DownloadArtifact {
    DownloadArtifact {
        url: url.to_string(),
        size_bytes,
        sha256: sha256.to_string(),
        archive,
        strip_components,
        entry_point: entry_point.map(str::to_string),
    }
}

/// Managed root for a provider. `{appData}` is expanded at install time.
fn managed_target(id: &str, version: &str) -> String {
    format!("{{appData}}/runtimes/{id}/{version}")
}

fn linux_providers() -> Vec<RuntimeProvider> {
    let mut umu = provider(
        "umu-launcher",
        Platform::Linux,
        ProviderKind::Launcher,
        "UMU Launcher",
        "1.4.4",
        "GPL-3.0",
        "https://github.com/Open-Wine-Components/umu-launcher",
        "Runs Windows games through a pinned Proton build and bootstraps the Steam Linux Runtime.",
    );
    umu.managed = true;
    umu.requires_consent = true;
    umu.download = Some(managed_download(
        "https://github.com/Open-Wine-Components/umu-launcher/releases/download/1.4.4/umu-launcher-1.4.4-zipapp.tar",
        430_080,
        "eb590691841f7fad3fc3ad8fd5db4ccb87849fe7948e62b28ece7a4ee48cc851",
        ArchiveKind::Tar,
        0,
        Some("umu/umu-run"),
    ));
    umu.install = Some(InstallSpec {
        kind: InstallKind::Extract,
        target: managed_target("umu-launcher", "1.4.4"),
        args: Vec::new(),
        inner_args: Vec::new(),
    });
    umu.detection = DetectionSpec {
        paths: vec![
            "{home}/.local/bin/umu-run".to_string(),
            "/usr/local/bin/umu-run".to_string(),
            "/usr/bin/umu-run".to_string(),
        ],
        managed: true,
        ..DetectionSpec::default()
    };
    umu.notes = vec![
        "UMU starts the game through the Steam Linux Runtime; Ludex never runs a Proton binary bare."
            .to_string(),
        "Installed per user under the Ludex data folder, no administrator rights needed."
            .to_string(),
    ];

    let mut proton = provider(
        "ge-proton",
        Platform::Linux,
        ProviderKind::Proton,
        "GE-Proton",
        "11-12",
        "GPL-3.0",
        "https://github.com/GloriousEggroll/proton-ge-custom",
        "Pinned Proton build with DXVK and VKD3D included; UMU uses it as PROTONPATH.",
    );
    proton.managed = true;
    proton.requires_consent = true;
    proton.arch = Some("x86_64".to_string());
    proton.download = Some(managed_download(
        "https://github.com/GloriousEggroll/proton-ge-custom/releases/download/GE-Proton11-12/GE-Proton11-12-x86_64.tar.gz",
        566_859_228,
        "0595159e417c3dbbfb7cc3ae0d3ed15a617cc607aa23ef756ea79029d7af5d70",
        ArchiveKind::TarGz,
        1,
        Some("proton"),
    ));
    proton.install = Some(InstallSpec {
        kind: InstallKind::Extract,
        target: managed_target("proton", "GE-Proton11-12"),
        args: Vec::new(),
        inner_args: Vec::new(),
    });
    proton.detection = DetectionSpec {
        managed: true,
        ..DetectionSpec::default()
    };
    proton.notes = vec![
        "Pinned so a later launch uses the same build. Update it explicitly in Settings → Compatibility."
            .to_string(),
        "DXVK and VKD3D ship inside Proton; do not install them separately."
            .to_string(),
    ];

    let mut wine = provider(
        "wine-staging",
        Platform::Linux,
        ProviderKind::Wine,
        "Wine staging",
        "10.x",
        "LGPL-2.1",
        "https://www.winehq.org",
        "Fallback for old or non-game Windows programs; not the recommended path for games.",
    );
    wine.detection = DetectionSpec {
        paths: vec![
            "{home}/.local/bin/wine".to_string(),
            "/usr/local/bin/wine".to_string(),
            "/usr/bin/wine".to_string(),
            "/usr/bin/wine64".to_string(),
        ],
        ..DetectionSpec::default()
    };
    wine.manual_steps = vec![super::ManualStep {
        label: "Install Wine from your distribution".to_string(),
        detail: "Use your package manager for wine-staging, or WineHQ's instructions. Ludex never runs a curl pipe."
            .to_string(),
        url: Some("https://wiki.winehq.org/Download".to_string()),
        command: None,
    }];

    let mut gamescope = provider(
        "gamescope",
        Platform::Linux,
        ProviderKind::Launcher,
        "Gamescope",
        "optional",
        "MIT",
        "https://github.com/ValveSoftware/gamescope",
        "Optional micro-compositor for fullscreen and scaling; never installed automatically.",
    );
    gamescope.detection = DetectionSpec {
        paths: vec!["/usr/bin/gamescope".to_string()],
        ..DetectionSpec::default()
    };

    let mut gamemode = provider(
        "gamemode",
        Platform::Linux,
        ProviderKind::Launcher,
        "GameMode",
        "optional",
        "BSD-3-Clause",
        "https://github.com/FeralInteractive/gamemode",
        "Optional CPU governor helper; never installed automatically.",
    );
    gamemode.detection = DetectionSpec {
        paths: vec!["/usr/bin/gamemoderun".to_string()],
        ..DetectionSpec::default()
    };

    let mut mangohud = provider(
        "mangohud",
        Platform::Linux,
        ProviderKind::Launcher,
        "MangoHud",
        "optional",
        "MIT",
        "https://github.com/flightlessmango/MangoHud",
        "Optional performance overlay; never installed automatically.",
    );
    mangohud.detection = DetectionSpec {
        paths: vec!["/usr/bin/mangohud".to_string()],
        ..DetectionSpec::default()
    };

    vec![umu, proton, wine, gamescope, gamemode, mangohud]
}

fn windows_providers() -> Vec<RuntimeProvider> {
    let mut vcrun_x64 = provider(
        "vcrun-2022-x64",
        Platform::Windows,
        ProviderKind::Redistributable,
        "Visual C++ 2015–2022 (x64)",
        "14.44.35211",
        "Microsoft EULA",
        "https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist",
        "The 64-bit Visual C++ runtime most Windows games ship as a prerequisite.",
    );
    vcrun_x64.managed = true;
    vcrun_x64.requires_admin = true;
    vcrun_x64.requires_consent = true;
    vcrun_x64.download = Some(managed_download(
        "https://aka.ms/vs/17/release/vc_redist.x64.exe",
        25_635_768,
        "cc0ff0eb1dc3f5188ae6300faef32bf5beeba4bdd6e8e445a9184072096b713b",
        ArchiveKind::Exe,
        0,
        None,
    ));
    vcrun_x64.install = Some(InstallSpec {
        kind: InstallKind::Installer,
        target: "{cache}".to_string(),
        args: vec![
            "/install".to_string(),
            "/quiet".to_string(),
            "/norestart".to_string(),
        ],
        inner_args: Vec::new(),
    });
    vcrun_x64.detection = DetectionSpec {
        registry: vec![
            RegistryCheck {
                key: "HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64".to_string(),
                value: "Installed".to_string(),
                expected: Some("1".to_string()),
                min_version: None,
            },
            RegistryCheck {
                key: "HKLM\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64".to_string(),
                value: "Version".to_string(),
                expected: None,
                min_version: Some("14.0".to_string()),
            },
        ],
        ..DetectionSpec::default()
    };

    let mut vcrun_x86 = provider(
        "vcrun-2022-x86",
        Platform::Windows,
        ProviderKind::Redistributable,
        "Visual C++ 2015–2022 (x86)",
        "14.44.35211",
        "Microsoft EULA",
        "https://learn.microsoft.com/cpp/windows/latest-supported-vc-redist",
        "The 32-bit Visual C++ runtime, needed only by 32-bit games.",
    );
    vcrun_x86.managed = true;
    vcrun_x86.requires_admin = true;
    vcrun_x86.requires_consent = true;
    vcrun_x86.download = Some(managed_download(
        "https://aka.ms/vs/17/release/vc_redist.x86.exe",
        13_953_392,
        "0c09f2611660441084ce0df425c51c11e147e6447963c3690f97e0b25c55ed64",
        ArchiveKind::Exe,
        0,
        None,
    ));
    vcrun_x86.install = Some(InstallSpec {
        kind: InstallKind::Installer,
        target: "{cache}".to_string(),
        args: vec![
            "/install".to_string(),
            "/quiet".to_string(),
            "/norestart".to_string(),
        ],
        inner_args: Vec::new(),
    });
    vcrun_x86.detection =
        DetectionSpec {
            registry: vec![
            RegistryCheck {
                key: "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x86"
                    .to_string(),
                value: "Installed".to_string(),
                expected: Some("1".to_string()),
                min_version: None,
            },
            RegistryCheck {
                key: "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x86"
                    .to_string(),
                value: "Version".to_string(),
                expected: None,
                min_version: Some("14.0".to_string()),
            },
        ],
            ..DetectionSpec::default()
        };

    let mut dotnet = provider(
        "dotnet-desktop-10",
        Platform::Windows,
        ProviderKind::Redistributable,
        ".NET Desktop Runtime 10",
        "10.0.15",
        "Microsoft EULA",
        "https://dotnet.microsoft.com/download/dotnet/10.0",
        "Desktop runtime for games built against .NET 8, 9 or 10.",
    );
    dotnet.managed = true;
    dotnet.requires_admin = true;
    dotnet.requires_consent = true;
    dotnet.download = Some(managed_download(
        "https://aka.ms/dotnet/10.0/windowsdesktop-runtime-win-x64.exe",
        60_066_584,
        "e61b030c1673edfa3c0844502eff83198c46af9e7df70dfc6e2efc5b11ea513a",
        ArchiveKind::Exe,
        0,
        None,
    ));
    dotnet.install = Some(InstallSpec {
        kind: InstallKind::Installer,
        target: "{cache}".to_string(),
        args: vec![
            "/install".to_string(),
            "/quiet".to_string(),
            "/norestart".to_string(),
        ],
        inner_args: Vec::new(),
    });
    dotnet.detection = DetectionSpec {
        registry: vec![RegistryCheck {
            key: "HKLM\\SOFTWARE\\dotnet\\Setup\\InstalledVersions\\x64\\sharedfx\\Microsoft.WindowsDesktop.App"
                .to_string(),
            value: String::new(),
            expected: None,
            min_version: None,
        }],
        ..DetectionSpec::default()
    };

    let mut directx = provider(
        "directx-jun2010",
        Platform::Windows,
        ProviderKind::Redistributable,
        "DirectX End-User Runtime (June 2010)",
        "9.29.1974",
        "Microsoft EULA",
        "https://www.microsoft.com/download/details.aspx?id=8109",
        "Legacy D3DX/DirectInput libraries some older games still need.",
    );
    directx.managed = true;
    directx.requires_admin = true;
    directx.requires_consent = true;
    directx.download = Some(managed_download(
        "https://download.microsoft.com/download/8/4/a/84a35bf1-dafe-4ae8-82af-ad2ae20b6b14/directx_Jun2010_redist.exe",
        100_275_120,
        "053f76dcbb28802e23341b6a787e3b0791c0fa5c8d4d011b1044172dbf89c73b",
        ArchiveKind::Exe,
        0,
        None,
    ));
    directx.install = Some(InstallSpec {
        kind: InstallKind::DirectXRedist,
        target: "{cache}/directx-jun2010".to_string(),
        args: vec!["/Q".to_string(), "/T:{extract}".to_string()],
        inner_args: vec!["/silent".to_string()],
    });
    directx.detection = DetectionSpec::default();
    directx.notes = vec![
        "DirectX has no reliable version key; Ludex records the install instead of guessing."
            .to_string(),
    ];

    let mut physx = provider(
        "physx",
        Platform::Windows,
        ProviderKind::Redistributable,
        "NVIDIA PhysX System Software",
        "9.23.1019",
        "NVIDIA EULA",
        "https://www.nvidia.com/en-us/drivers/physx/",
        "Only needed by games that declare it; Ludex links to NVIDIA instead of redistributing it.",
    );
    physx.detection = DetectionSpec {
        paths: vec![
            "C:\\Program Files (x86)\\NVIDIA Corporation\\PhysX\\Common\\PhysXCore.dll".to_string(),
        ],
        ..DetectionSpec::default()
    };
    physx.manual_steps = vec![super::ManualStep {
        label: "Install PhysX from NVIDIA".to_string(),
        detail: "Download the System Software installer from NVIDIA and run it. It is not redistributed with Ludex."
            .to_string(),
        url: Some("https://www.nvidia.com/en-us/drivers/physx/".to_string()),
        command: None,
    }];

    vec![vcrun_x64, vcrun_x86, dotnet, directx, physx]
}

fn macos_providers() -> Vec<RuntimeProvider> {
    let mut rosetta = provider(
        "rosetta-2",
        Platform::Macos,
        ProviderKind::Translation,
        "Rosetta 2",
        "2",
        "Apple licence",
        "https://support.apple.com/en-us/102527",
        "Apple's x86-64 translation layer, required to run Intel-only wrappers on Apple Silicon.",
    );
    rosetta.requires_admin = true;
    rosetta.arch = Some("aarch64".to_string());
    rosetta.detection = DetectionSpec {
        paths: vec!["/Library/Apple/usr/libexec/oah/translate".to_string()],
        ..DetectionSpec::default()
    };
    rosetta.manual_steps = vec![super::ManualStep {
        label: "Install Rosetta 2 in Terminal".to_string(),
        detail: "macOS asks for your password. Ludex never runs this command for you.".to_string(),
        url: None,
        command: Some("softwareupdate --install-rosetta --agree-to-license".to_string()),
    }];

    let mut whisky = provider(
        "whisky",
        Platform::Macos,
        ProviderKind::Wrapper,
        "Whisky (maintained fork)",
        "11.x",
        "GPL-3.0",
        "https://github.com/frankea/Whisky",
        "Wine 11 wrapper for Apple Silicon (macOS 15+), signed by its maintainer.",
    );
    whisky.arch = Some("aarch64".to_string());
    whisky.detection = DetectionSpec {
        app_bundles: vec!["/Applications/Whisky.app".to_string()],
        ..DetectionSpec::default()
    };
    whisky.manual_steps = vec![super::ManualStep {
        label: "Download Whisky from the maintained fork".to_string(),
        detail: "Download the signed release from GitHub and move Whisky to Applications."
            .to_string(),
        url: Some("https://github.com/frankea/Whisky/releases".to_string()),
        command: None,
    }];
    whisky.notes = vec![
        "The original Whisky was archived in 2025; this is the maintained fork.".to_string(),
        "Requires macOS 15 or newer.".to_string(),
    ];

    let mut heroic = provider(
        "heroic",
        Platform::Macos,
        ProviderKind::Wrapper,
        "Heroic Games Launcher",
        "2.x",
        "GPL-3.0",
        "https://heroicgameslauncher.com",
        "Open-source launcher with its own Wine manager; works on Apple Silicon and Intel.",
    );
    heroic.detection = DetectionSpec {
        app_bundles: vec!["/Applications/Heroic.app".to_string()],
        ..DetectionSpec::default()
    };
    heroic.manual_steps = vec![super::ManualStep {
        label: "Download Heroic".to_string(),
        detail: "Install Heroic from its official site, then let its Wine Manager fetch a build."
            .to_string(),
        url: Some("https://heroicgameslauncher.com".to_string()),
        command: None,
    }];

    let mut crossover = provider(
        "crossover",
        Platform::Macos,
        ProviderKind::Wrapper,
        "CrossOver",
        "26",
        "Proprietary (paid)",
        "https://www.codeweavers.com/crossover",
        "Paid commercial Wine wrapper. CrossOver 26 is the last release that supports Intel Macs.",
    );
    crossover.detection = DetectionSpec {
        app_bundles: vec!["/Applications/CrossOver.app".to_string()],
        ..DetectionSpec::default()
    };
    crossover.manual_steps = vec![super::ManualStep {
        label: "Buy and install CrossOver".to_string(),
        detail:
            "Purchase from CodeWeavers and install it. Ludex never bundles or downloads CrossOver."
                .to_string(),
        url: Some("https://www.codeweavers.com/crossover".to_string()),
        command: None,
    }];
    crossover.notes = vec![
        "Intel Macs: macOS 26/27/28 end-of-life risk. CrossOver 26 is the last Intel-capable release."
            .to_string(),
    ];

    let mut sikarugir = provider(
        "sikarugir",
        Platform::Macos,
        ProviderKind::Wrapper,
        "Sikarugir",
        "1.x",
        "GPL-3.0",
        "https://github.com/Sikarugir-App/Sikarugir",
        "Community Wine wrapper for macOS.",
    );
    sikarugir.detection = DetectionSpec {
        app_bundles: vec!["/Applications/Sikarugir.app".to_string()],
        ..DetectionSpec::default()
    };
    sikarugir.manual_steps = vec![super::ManualStep {
        label: "Download Sikarugir".to_string(),
        detail: "Download a release from GitHub and move it to Applications.".to_string(),
        url: Some("https://github.com/Sikarugir-App/Sikarugir".to_string()),
        command: None,
    }];

    let mut gptk = provider(
        "gptk",
        Platform::Macos,
        ProviderKind::Translation,
        "Game Porting Toolkit / D3DMetal",
        "user-supplied",
        "Apple evaluation licence — non-redistributable",
        "https://developer.apple.com/games/game-porting-toolkit/",
        "Apple's translation layer. You download it with your own Apple ID; Ludex only verifies and records the DMG you import.",
    );
    gptk.arch = Some("aarch64".to_string());
    gptk.install = Some(InstallSpec {
        kind: InstallKind::Import,
        target: String::new(),
        args: Vec::new(),
        inner_args: Vec::new(),
    });
    gptk.manual_steps = vec![super::ManualStep {
        label: "Download Game Porting Toolkit from Apple".to_string(),
        detail: "Sign in with your Apple ID, download the DMG, then import it here. Ludex never copies or bundles it; the licence is evaluation-only."
            .to_string(),
        url: Some("https://developer.apple.com/games/game-porting-toolkit/".to_string()),
        command: None,
    }];
    gptk.notes = vec![
        "Apple Silicon only; GPTK/D3DMetal does not run on Intel Macs.".to_string(),
        "Ludex records the DMG path and SHA-256, and re-verifies the file before using it."
            .to_string(),
    ];

    vec![rosetta, whisky, heroic, crossover, sikarugir, gptk]
}

/// Build the shipped manifest. Tests validate it structurally.
pub fn build() -> RuntimeCatalog {
    let mut providers = Vec::new();
    providers.extend(linux_providers());
    providers.extend(windows_providers());
    providers.extend(macos_providers());

    let catalog = RuntimeCatalog {
        schema_version: 1,
        manifest_version: "2026.11.12".to_string(),
        generated_at: "2026-11-12T00:00:00Z".to_string(),
        refresh: ManifestRefresh {
            enabled: false,
            reason: "Remote refresh stays off until a signing key ships; the app fails closed rather than trusting an unsigned manifest."
                .to_string(),
        },
        max_download_bytes: MAX_DOWNLOAD_BYTES,
        providers,
        notices: vec![
            "Kernel-level anti-cheat (EAC/BattlEye without developer Proton support) does not run under Wine or Proton and can put accounts at risk. Ludex never bypasses anti-cheat."
                .to_string(),
            "Ludex never bundles or redistributes GPTK, CrossOver, Steam, games or GPU drivers."
                .to_string(),
            "No runtime is installed without an explicit click, and every checksum mismatch aborts before anything runs."
                .to_string(),
        ],
    };
    debug_assert!(
        validate(&catalog).is_empty(),
        "runtime manifest problems: {:?}",
        validate(&catalog)
    );
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_manifest_is_structurally_valid() {
        let catalog = build();
        let problems = validate(&catalog);
        assert!(problems.is_empty(), "manifest problems: {problems:?}");
        assert!(catalog.providers.iter().any(|p| p.id == "umu-launcher"));
        assert!(catalog.providers.iter().any(|p| p.id == "ge-proton"));
        assert!(catalog.providers.iter().any(|p| p.id == "vcrun-2022-x64"));
        assert!(catalog.providers.iter().any(|p| p.id == "rosetta-2"));
        assert!(catalog.providers.iter().any(|p| p.id == "gptk"));
    }

    #[test]
    fn pinned_versions_are_exact_and_checksummed() {
        let catalog = build();
        let umu = catalog
            .providers
            .iter()
            .find(|provider| provider.id == "umu-launcher")
            .unwrap();
        let download = umu.download.as_ref().unwrap();
        assert_eq!(download.size_bytes, 430_080);
        assert_eq!(download.sha256.len(), 64);
        assert_eq!(download.entry_point.as_deref(), Some("umu/umu-run"));
        assert!(!umu.version.contains('*'));
    }

    #[test]
    fn allow_list_accepts_known_https_hosts() {
        assert!(check_download_url(
            "https://github.com/Open-Wine-Components/umu-launcher/releases/download/1.4.4/file.tar"
        )
        .is_ok());
        assert!(check_download_url("https://aka.ms/vs/17/release/vc_redist.x64.exe").is_ok());
        assert!(check_download_url(
            "https://download.microsoft.com/download/8/4/a/84a35bf1/directx.exe"
        )
        .is_ok());
    }

    #[test]
    fn allow_list_rejects_unsafe_urls() {
        let rejected = [
            "http://github.com/foo/bar",
            "https://evil.example.com/umu.tar",
            "https://github.com.evil.example/umu.tar",
            "https://github.com:8443/umu.tar",
            "https://user:pass@github.com/umu.tar",
            "file:///etc/passwd",
            "data:text/plain,hello",
            "https://github.com/",
            "https://github.com",
            "not a url",
        ];
        for url in rejected {
            assert!(
                check_download_url(url).is_err(),
                "{url} should be rejected by the allow-list"
            );
        }
    }

    #[test]
    fn validation_rejects_tampered_checksum_and_unknown_host() {
        let mut catalog = build();
        let provider = catalog
            .providers
            .iter_mut()
            .find(|provider| provider.id == "umu-launcher")
            .unwrap();
        provider.download.as_mut().unwrap().sha256 = "abc".to_string();
        provider.download.as_mut().unwrap().url = "https://evil.example.com/umu.tar".to_string();
        let problems = validate(&catalog);
        assert!(problems.iter().any(|problem| problem.contains("sha256")));
        assert!(problems
            .iter()
            .any(|problem| problem.contains("unknown host")));
    }
}
