//! Guided compatibility runtime installer (#178): "Make it run".
//!
//! The feature is deliberately split from [`crate::compat`] and
//! [`crate::launch`]: detection there answers "which tool can start this
//! file", while this module answers "what does the machine still lack, what
//! exactly would Ludex download, and may it do so".
//!
//! Flow:
//!   1. [`catalog`] is the shipped, version-pinned provider manifest. Every
//!      managed provider carries a URL on an allow-listed HTTPS host, a size
//!      and a SHA-256.
//!   2. [`detect`] probes the machine (filesystem, registry, app bundles)
//!      through the [`detect::Probe`] trait, so tests inject fixtures instead
//!      of touching the host.
//!   3. [`plan`] turns detections plus a game's declared prerequisites into
//!      issues and a numbered plan. Nothing is downloaded here.
//!   4. `make_it_run_apply` in [`commands`] requires explicit consent, then
//!      [`install`] downloads, verifies the checksum and only then extracts or
//!      runs a signed installer. Progress is emitted on `runtime://progress`.
//!
//! Security rules enforced in code, not only in the UI:
//!   - downloads only start from `https://` URLs whose host is in
//!     [`catalog::ALLOWED_HOSTS`], and redirects are checked with the same
//!     allow-list;
//!   - the expected size and SHA-256 come from the catalog, never from the
//!     web view; a mismatch aborts before anything executes;
//!   - no shell is used for any command: installers are spawned with a fixed
//!     argument list from the catalog;
//!   - a provider with no allow-listed download is a documented manual step,
//!     never an attempt to fetch an arbitrary URL.

pub mod catalog;
pub mod commands;
pub mod detect;
pub mod install;
pub mod plan;

use serde::{Deserialize, Serialize};

/// Largest single runtime artifact Ludex will fetch (2 GiB).
pub const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Free space kept in reserve on the runtime volume.
pub const DISK_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

/// Operating system a provider targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
    Macos,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Macos
        } else {
            Platform::Linux
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Windows => "windows",
            Platform::Macos => "macos",
        }
    }
}

/// What role a provider plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    /// UMU, Heroic, Whisky: something that hosts the game.
    Launcher,
    /// A Proton build.
    Proton,
    /// Plain Wine.
    Wine,
    /// VC++/.NET/DirectX/PhysX.
    Redistributable,
    /// Rosetta 2 or GPTK/D3DMetal.
    Translation,
    /// A macOS wrapper such as CrossOver.
    Wrapper,
}

/// How an artifact is unpacked after verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArchiveKind {
    /// `.tar.gz` / `.tgz`
    TarGz,
    /// Uncompressed `.tar`
    Tar,
    /// `.zip`
    Zip,
    /// A Windows `.exe` installer, run rather than unpacked.
    Exe,
    /// A macOS `.dmg`, imported by the user, never copied.
    Dmg,
}

/// What applying a provider does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallKind {
    /// Unpack an archive into the managed runtime folder.
    Extract,
    /// Run a signed redistributable installer.
    Installer,
    /// Extract the DirectX redistributable, then run its inner `DXSETUP.exe`.
    DirectXRedist,
    /// Record a user-supplied artifact (GPTK DMG) without copying it.
    Import,
}

/// One verified download. The values are compile-time constants in
/// [`catalog`]; nothing here is ever supplied by the web view.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadArtifact {
    pub url: String,
    pub size_bytes: u64,
    /// Lower-case hex SHA-256.
    pub sha256: String,
    pub archive: ArchiveKind,
    /// Leading path components to drop when extracting (a tarball often wraps
    /// everything in `GE-Proton11-12/`).
    pub strip_components: usize,
    /// Executable relative to the install root, when the provider is a tool
    /// rather than a redistributable.
    pub entry_point: Option<String>,
}

/// How a provider is installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallSpec {
    pub kind: InstallKind,
    /// `{appData}/runtimes/<id>/{version}` for managed files.
    pub target: String,
    /// Fixed arguments; never built from user input. `{extract}` is replaced
    /// with the temporary extraction folder.
    pub args: Vec<String>,
    /// Arguments for the inner `DXSETUP.exe`, when applicable.
    pub inner_args: Vec<String>,
}

/// One registry value checked during detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCheck {
    pub key: String,
    pub value: String,
    /// Exact expected value, when the key stores `Installed = 1`.
    pub expected: Option<String>,
    /// Dotted numeric minimum, used for runtime versions.
    pub min_version: Option<String>,
}

/// Where detection looks for a provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectionSpec {
    /// Files that prove the provider is present. Placeholders: `{home}`,
    /// `{appData}`, `{systemRoot}`.
    pub paths: Vec<String>,
    /// Directories that prove presence (an app folder, a runtime root).
    pub dirs: Vec<String>,
    /// macOS `.app` bundles.
    pub app_bundles: Vec<String>,
    pub registry: Vec<RegistryCheck>,
    /// Also accept this provider's recorded Ludex-managed folder.
    pub managed: bool,
}

/// A step only the user can perform (external download, licence, OS install).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualStep {
    pub label: String,
    pub detail: String,
    /// HTTPS link the UI may open in the user's browser.
    pub url: Option<String>,
    /// Exact command to display. Never executed by Ludex.
    pub command: Option<String>,
}

/// One runtime provider in the shipped manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProvider {
    pub id: String,
    pub platform: Platform,
    pub kind: ProviderKind,
    pub name: String,
    /// Pinned version, shown on the game page and in the audit log.
    pub version: String,
    pub licence: String,
    pub homepage: String,
    pub summary: String,
    /// Ludex downloads, verifies and can remove this provider.
    pub managed: bool,
    /// Machine-wide installers that need administrator rights.
    pub requires_admin: bool,
    /// Third-party licence the user must accept before the download.
    pub requires_consent: bool,
    pub download: Option<DownloadArtifact>,
    pub install: Option<InstallSpec>,
    pub detection: DetectionSpec,
    pub manual_steps: Vec<ManualStep>,
    pub notes: Vec<String>,
    /// When set, the provider only applies to this CPU architecture.
    pub arch: Option<String>,
}

/// Manifest refresh state. Remote refresh stays off until an Ed25519 key is
/// shipped; the app then verifies the signature before accepting new values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestRefresh {
    pub enabled: bool,
    pub reason: String,
}

/// The versioned manifest serialised by `runtime_catalog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCatalog {
    pub schema_version: u32,
    pub manifest_version: String,
    pub generated_at: String,
    pub refresh: ManifestRefresh,
    pub max_download_bytes: u64,
    pub providers: Vec<RuntimeProvider>,
    pub notices: Vec<String>,
}

/// A runtime Ludex installed (or an import it recorded), persisted per
/// machine in the desktop index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInstallRow {
    pub provider_id: String,
    pub platform: String,
    pub version: String,
    /// Managed install folder, or the imported DMG path. Empty for registry
    /// redistributables.
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub installed_at: String,
}

/// The runtime chosen for one game, so later launches are reproducible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeGamePrefs {
    pub game_id: i64,
    pub provider_id: String,
    pub provider_version: String,
    pub prefix_path: String,
    pub prerequisites: Vec<String>,
    pub updated_at: String,
}

/// One line of the local audit log. It never leaves this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAuditEvent {
    pub id: String,
    pub game_id: i64,
    pub provider_id: String,
    pub version: String,
    /// `install`, `configure`, `import`, `remove`, `select`, `cancel`,
    /// `needs-elevation`, `declined`.
    pub action: String,
    /// `ok`, `failed`, `cancelled`, `manual`.
    pub outcome: String,
    pub detail: String,
    pub created_at: String,
}

/// Shipped manifest. The generated date is bumped with the pinned versions.
pub fn catalog() -> RuntimeCatalog {
    catalog::build()
}

/// One in-flight `make_it_run_apply` run, kept in [`crate::state::AppState`]
/// so the UI can cancel it and a second apply cannot start concurrently.
#[derive(Debug)]
pub struct RuntimeRun {
    pub cancel: tokio_util::sync::CancellationToken,
}
