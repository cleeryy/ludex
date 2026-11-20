//! Per-game launch auto-configuration (#297).
//!
//! A small rules engine turns what Ludex already knows about a game (its
//! executable, engine markers, a known GAMEID, how it entered the library, the
//! GPU/driver family and the host tools) into one [`LaunchPlan`]: the runtime,
//! the Proton build preference, the environment, the prefix policy, the
//! prerequisite recipe and the wrappers.
//!
//! The rules come from a [`RulesProvider`]. [`BuiltinRules`] is the shipped,
//! deliberately small set; a data-backed provider (the rules-database lane,
//! #301) can replace it without touching the engine.
//!
//! The plan is what the user reads: one sentence ("Launching with UMU and
//! GE-Proton, HDR off, GameMode on."). It is stored per game under
//! `<appDataDir>/launch-plans/<gameId>.json` so a working configuration can be
//! reproduced and exported, and it is written into the launch log before the
//! wrapper starts.
//!
//! Nothing here launches a game. The launch path in [`crate::launch`] applies
//! the plan; the pure decisions are unit tested with fixtures.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::linux_gaming::{DisplayProfile, LaunchMode, LaunchOptions};

/// Plan schema version, bumped when a stored plan stops being readable.
pub const PLAN_VERSION: u32 = 1;
/// Expected startup budget recorded in the plan, shown to support and in the
/// sentence data. It is **not** the failure decision: the authoritative
/// classifier and budget live in [`crate::launch_recovery`] (#298).
pub const DEFAULT_FRAME_BUDGET_SECS: u32 = 5;
/* ---------------------------------------------------------------------- */
/* Inputs                                                                 */
/* ---------------------------------------------------------------------- */

/// A game engine the engine-marker scan can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    Unreal4,
    Unreal5,
    Unity,
    Godot,
    /// A known engine marker was not recognised.
    Other,
}

/// How a game entered the local library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArchiveOrigin {
    /// Installed by Ludex from an archive.
    Ludex,
    /// An existing folder linked in place (#129).
    Adopted,
    /// Imported from another client and launched through its URI.
    Launcher,
}

/// GPU vendor read from the DRM device, when it is recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    /// A vendor id was read but is not one of the three above.
    Other,
    Unknown,
}

/// What the kernel driver is, in the spelling the plan records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DriverFamily {
    /// `nvidia`: the proprietary driver.
    NvidiaProprietary,
    /// `nouveau`: the open-source NVIDIA driver, no usable Vulkan.
    Nouveau,
    /// `amdgpu` with Mesa (RADV) or `amdgpu-pro`/AMDVLK.
    Amd,
    /// `i915`/`xe` with Mesa (ANV).
    Intel,
    /// No DRM driver name was readable.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuProfile {
    pub vendor: GpuVendor,
    pub driver: DriverFamily,
    /// Raw driver name from sysfs (`nvidia`, `amdgpu`, `nouveau`, …).
    pub driver_name: String,
}

impl Default for GpuProfile {
    fn default() -> Self {
        Self {
            vendor: GpuVendor::Unknown,
            driver: DriverFamily::Unknown,
            driver_name: String::new(),
        }
    }
}

/// A compatibility tool the plan may pick, without the full scan record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCandidate {
    /// `proton:/path`, `umu:/path`, `wine:/path`.
    pub id: String,
    pub name: String,
    /// `umu`, `proton` or `wine`.
    pub kind: String,
    pub proton_path: Option<String>,
}

/// Everything the rules may look at. Serialised for the input fingerprint, so
/// field order and values fully describe a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanInput {
    pub game_id: i64,
    pub title: String,
    pub executable: Option<String>,
    /// True when the launch target is a Windows PE (or ends in `.exe`).
    pub windows_executable: bool,
    pub exe_machine: Option<String>,
    pub engine: Option<Engine>,
    /// GAMEID UMU should use, when one is known.
    pub known_game_id: Option<String>,
    pub archive_origin: ArchiveOrigin,
    pub gpu: GpuProfile,
    /// `gamemoderun` is installed.
    pub gamemode: bool,
    /// MangoHud is installed.
    pub mangohud: bool,
    /// `winetricks` is installed.
    pub winetricks: bool,
    /// Redistributable ids found in the install folder ([`crate::runtime::plan::PREREQUISITE_IDS`]).
    pub declared_prerequisites: Vec<String>,
    /// The stored per-game display profile. The default profile means "no
    /// decision yet"; anything else is a manual display choice.
    pub display: DisplayProfile,
    /// True when the saved compatibility options already decide the launch.
    /// The plan is then informational only.
    pub manual_override: bool,
    pub host_arch: String,
    pub tools: Vec<ToolCandidate>,
}

impl PlanInput {
    /// Short, stable fingerprint of everything the plan depends on.
    pub fn fingerprint(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let digest = Sha256::digest(bytes);
        hex::encode(&digest[..12])
    }
}

/* ---------------------------------------------------------------------- */
/* Rules                                                                  */
/* ---------------------------------------------------------------------- */

/// What a rule matches. Empty vectors mean "no constraint".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Match {
    pub origins: Vec<ArchiveOrigin>,
    pub windows_executable: Option<bool>,
    pub engines: Vec<Engine>,
    /// Known GAMEIDs (UMU ids or Steam app ids) a curated rule targets.
    pub known_game_ids: Vec<String>,
    pub gpu_vendors: Vec<GpuVendor>,
    pub drivers: Vec<DriverFamily>,
    /// Any-of: the install declares one of these prerequisite ids.
    pub declared_prerequisites: Vec<String>,
    pub gamemode: Option<bool>,
}

/// How the game is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PlanRuntime {
    /// `umu-run` with a Proton build.
    #[default]
    Umu,
    /// The file runs as-is.
    Native,
    /// `wine` without UMU.
    Wine,
    /// The game is opened through its own launcher URI.
    LauncherUri,
}

impl PlanRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanRuntime::Umu => "umu",
            PlanRuntime::Native => "native",
            PlanRuntime::Wine => "wine",
            PlanRuntime::LauncherUri => "launcher-uri",
        }
    }

    fn launch_mode(self) -> LaunchMode {
        match self {
            PlanRuntime::Umu => LaunchMode::Umu,
            PlanRuntime::Native => LaunchMode::Native,
            PlanRuntime::Wine => LaunchMode::Proton,
            PlanRuntime::LauncherUri => LaunchMode::Auto,
        }
    }
}

/// Which Proton build the plan prefers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ProtonPreference {
    /// The pinned GE-Proton build Ludex manages.
    #[default]
    Pinned,
    /// A build the user already has in Steam.
    Steam,
    /// Any installed build; the compat scan picks.
    Any,
    /// One build by name, matching [`ToolCandidate::name`].
    Build(String),
}

impl ProtonPreference {
    fn label(&self) -> String {
        match self {
            ProtonPreference::Pinned => "GE-Proton".to_string(),
            ProtonPreference::Steam => "Steam Proton".to_string(),
            ProtonPreference::Any => "the best Proton build".to_string(),
            ProtonPreference::Build(name) => name.clone(),
        }
    }
}

/// Prefix policy. Only one policy ships today; the enum keeps the plan
/// explicit so #299/#300 can extend it without changing the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PrefixPolicy {
    /// `{installPath}/.prefix`, one per game.
    #[default]
    PerGame,
}

impl PrefixPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            PrefixPolicy::PerGame => "per-game",
        }
    }
}

/// One prerequisite recipe step, named for people and mapped to verbs
/// internally. The primary UI never shows the verb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prerequisite {
    /// Stable id (`vcrun`, `directx`).
    pub id: String,
    /// Human label ("Visual C++ 2015–2022 runtime").
    pub label: String,
    /// Internal winetricks verbs; never rendered in the primary UI.
    pub verbs: Vec<String>,
}

/// Map a stable prerequisite id to its recipe. `None` means the id has no
/// non-interactive recipe (PhysX ships only from NVIDIA).
pub fn recipe_for(id: &str) -> Option<Prerequisite> {
    match id {
        "vcrun" => Some(Prerequisite {
            id: "vcrun".to_string(),
            label: "Visual C++ 2015–2022 runtime".to_string(),
            verbs: vec!["vcrun2019".to_string()],
        }),
        "directx" => Some(Prerequisite {
            id: "directx".to_string(),
            label: "DirectX 9 runtime".to_string(),
            verbs: vec!["d3dx9".to_string()],
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Wrappers {
    /// Run inside Gamescope (from the display profile).
    pub gamescope: bool,
    /// Wrap the entry point with `gamemoderun`.
    pub gamemode: bool,
    /// Set `MANGOHUD=1`.
    pub mangohud: bool,
}

/// What one rule changes. Every field is optional; later rules win.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlanPatch {
    pub runtime: Option<PlanRuntime>,
    pub proton: Option<ProtonPreference>,
    /// Copy [`PlanInput::known_game_id`] into `GAMEID` once every rule ran.
    pub game_id_from_input: bool,
    pub env: Vec<(String, String)>,
    pub prerequisites: Vec<String>,
    pub gamemode: Option<bool>,
    pub mangohud: Option<bool>,
    pub first_frame_budget_secs: Option<u32>,
    pub note: Option<String>,
}

/// One declarative rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    /// Stable id, recorded in the plan and the launch log.
    pub id: String,
    /// Ascending order; equal priorities keep provider order.
    pub priority: i32,
    /// One sentence for the docs/advanced view.
    pub summary: String,
    #[serde(rename = "when")]
    pub when: Match,
    pub apply: PlanPatch,
}

impl Rule {
    fn matches(&self, input: &PlanInput) -> bool {
        let when = &self.when;
        if !when.origins.is_empty() && !when.origins.contains(&input.archive_origin) {
            return false;
        }
        if let Some(windows) = when.windows_executable {
            if windows != input.windows_executable {
                return false;
            }
        }
        if !when.engines.is_empty()
            && !input
                .engine
                .map(|engine| when.engines.contains(&engine))
                .unwrap_or(false)
        {
            return false;
        }
        if !when.known_game_ids.is_empty()
            && !input
                .known_game_id
                .as_ref()
                .map(|game_id| when.known_game_ids.contains(game_id))
                .unwrap_or(false)
        {
            return false;
        }
        if !when.gpu_vendors.is_empty() && !when.gpu_vendors.contains(&input.gpu.vendor) {
            return false;
        }
        if !when.drivers.is_empty() && !when.drivers.contains(&input.gpu.driver) {
            return false;
        }
        if !when.declared_prerequisites.is_empty()
            && !when.declared_prerequisites.iter().any(|id| {
                input
                    .declared_prerequisites
                    .iter()
                    .any(|declared| declared == id)
            })
        {
            return false;
        }
        if let Some(gamemode) = when.gamemode {
            if gamemode != input.gamemode {
                return false;
            }
        }
        true
    }
}

/// Where the rules come from.
///
/// The built-in provider is the shipped set. A data-backed provider (#301)
/// implements the same trait and is the only thing that changes.
pub trait RulesProvider {
    fn rules(&self) -> Vec<Rule>;

    /// Version recorded in the plan and the log (`builtin-1`).
    fn version(&self) -> String {
        "builtin-1".to_string()
    }
}

/// The shipped rules.
///
/// Deliberately few: each one is a general decision Ludex can justify without
/// a per-title database. The acceptance-title rules belong to the data
/// provider (#301); this set stays engine/host based.
pub struct BuiltinRules;

impl RulesProvider for BuiltinRules {
    fn rules(&self) -> Vec<Rule> {
        use ArchiveOrigin as O;
        use DriverFamily as D;
        use Engine as E;
        use GpuVendor as V;
        use PlanRuntime as R;

        vec![
            Rule {
                id: "launcher-uri".to_string(),
                priority: 0,
                summary: "An imported entry launches through its own launcher URI; no Wine layer is added."
                    .to_string(),
                when: Match {
                    origins: vec![O::Launcher],
                    ..Match::default()
                },
                apply: PlanPatch {
                    runtime: Some(R::LauncherUri),
                    proton: Some(ProtonPreference::Any),
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "native-build".to_string(),
                priority: 1,
                summary: "A non-PE executable runs natively; no compatibility runtime is involved."
                    .to_string(),
                when: Match {
                    windows_executable: Some(false),
                    ..Match::default()
                },
                apply: PlanPatch {
                    runtime: Some(R::Native),
                    proton: Some(ProtonPreference::Any),
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "windows-exe-umu".to_string(),
                priority: 10,
                summary: "A Windows executable runs through UMU with the pinned GE-Proton build (#89)."
                    .to_string(),
                when: Match {
                    windows_executable: Some(true),
                    origins: vec![O::Ludex, O::Adopted],
                    ..Match::default()
                },
                apply: PlanPatch {
                    runtime: Some(R::Umu),
                    proton: Some(ProtonPreference::Pinned),
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "steam-game-id".to_string(),
                priority: 20,
                summary: "A known Steam app id becomes UMU's GAMEID so protonfixes and the prefix match the released game."
                    .to_string(),
                when: Match {
                    windows_executable: Some(true),
                    ..Match::default()
                },
                apply: PlanPatch {
                    game_id_from_input: true,
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "declared-vcrun".to_string(),
                priority: 30,
                summary: "The game ships a Visual C++ redistributable, so the prefix gets the matching runtime before the first launch."
                    .to_string(),
                when: Match {
                    windows_executable: Some(true),
                    declared_prerequisites: vec!["vcrun".to_string()],
                    ..Match::default()
                },
                apply: PlanPatch {
                    prerequisites: vec!["vcrun".to_string()],
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "declared-directx".to_string(),
                priority: 31,
                summary: "The game ships a June 2010 DirectX redistributable, so DirectX 9 is installed in the prefix first."
                    .to_string(),
                when: Match {
                    windows_executable: Some(true),
                    declared_prerequisites: vec!["directx".to_string()],
                    ..Match::default()
                },
                apply: PlanPatch {
                    prerequisites: vec!["directx".to_string()],
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "engine-shader-budget".to_string(),
                priority: 40,
                summary: "Unreal and Unity compile shaders on the first run; the no-first-frame budget is raised to 15 s."
                    .to_string(),
                when: Match {
                    windows_executable: Some(true),
                    engines: vec![E::Unreal4, E::Unreal5, E::Unity],
                    ..Match::default()
                },
                apply: PlanPatch {
                    first_frame_budget_secs: Some(15),
                    note: Some(
                        "The first launch compiles shaders, so a slow first frame is expected."
                            .to_string(),
                    ),
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "gamemode-if-installed".to_string(),
                priority: 50,
                summary: "GameMode is applied to every game when gamemoderun is installed."
                    .to_string(),
                when: Match {
                    windows_executable: Some(true),
                    gamemode: Some(true),
                    ..Match::default()
                },
                apply: PlanPatch {
                    gamemode: Some(true),
                    ..PlanPatch::default()
                },
            },
            Rule {
                id: "nouveau-no-vulkan".to_string(),
                priority: 60,
                summary: "Nouveau exposes no usable Vulkan driver, so the plan warns instead of pretending DXVK will start."
                    .to_string(),
                when: Match {
                    gpu_vendors: vec![V::Nvidia],
                    drivers: vec![D::Nouveau],
                    ..Match::default()
                },
                apply: PlanPatch {
                    note: Some(
                        "The open-source Nouveau driver has no usable Vulkan support; install the NVIDIA proprietary driver before playing."
                            .to_string(),
                    ),
                    ..PlanPatch::default()
                },
            },
        ]
    }
}

/* ---------------------------------------------------------------------- */
/* Plan                                                                   */
/* ---------------------------------------------------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanSource {
    /// The rules produced the plan and Ludex applies it.
    Auto,
    /// The user saved compatibility options; the rules are informational.
    Manual,
}

/// The complete launch plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub version: u32,
    pub game_id: i64,
    pub title: String,
    pub runtime: PlanRuntime,
    /// False when the game has no recorded executable (an adopted folder whose
    /// launch target is only resolved at play time). The runtime is then left
    /// to the existing auto rule instead of forcing one.
    pub executable_known: bool,
    pub proton: ProtonPreference,
    /// The detected tool id this preference resolved to, when one matched.
    pub tool_id: Option<String>,
    pub game_id_env: Option<String>,
    pub env: BTreeMap<String, String>,
    pub prefix: PrefixPolicy,
    pub prerequisites: Vec<Prerequisite>,
    pub wrappers: Wrappers,
    pub display: DisplayProfile,
    /// Expected startup time for this game (5 s, 15 s when the engine compiles
    /// shaders). Informational: #298 decides a failure with its own documented
    /// 30 s budget.
    pub first_frame_budget_secs: u32,
    pub notes: Vec<String>,
    pub rules: Vec<String>,
    pub rules_version: String,
    pub source: PlanSource,
    /// One sentence for people.
    pub sentence: String,
    pub generated_at: String,
}

impl LaunchPlan {
    /// The launch mode this plan asks for, unless the user overrode it.
    pub fn launch_mode(&self) -> LaunchMode {
        self.runtime.launch_mode()
    }
}

/// Why a plan failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    /// The process tree ended inside the budget.
    NoFirstFrame,
    /// A known fatal exit code, after the budget.
    FatalExit,
}

/// A failed auto-configured launch. #298 consumes it for the auto-revert path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFailure {
    pub kind: FailureKind,
    pub exit_code: u32,
    pub seconds: u64,
    pub budget_secs: u32,
    pub hint: String,
}

impl PlanFailure {
    /// Describe the failure the #298 classifier already decided.
    ///
    /// The plan never re-classifies a session: `fatal` is
    /// [`crate::launch_recovery::is_fatal_exit`], `budgetSecs` is the recovery
    /// budget, so the counters and this event can never disagree.
    pub fn from_recovery(
        exit_code: Option<u32>,
        seconds: u64,
        budget_secs: u32,
        fatal: bool,
    ) -> Self {
        let code = exit_code.unwrap_or(0);
        Self {
            kind: if fatal {
                FailureKind::FatalExit
            } else {
                FailureKind::NoFirstFrame
            },
            exit_code: code,
            seconds,
            budget_secs,
            hint: failure_hint(code),
        }
    }
}

/// Payload of `launch://plan-failed` (#297 emits, #298 consumes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFailedEvent {
    pub game_id: i64,
    pub session_id: String,
    pub plan_sentence: String,
    pub rules: Vec<String>,
    pub failure: PlanFailure,
}

/// A short explanation of a fatal exit code, matching the launch-log hints.
pub fn failure_hint(code: u32) -> String {
    match code {
        0 => "the compatibility tool exited before the game started; check the launch log"
            .to_string(),
        1 => "the environment or the compatibility tool failed before the game started; check the launch log"
            .to_string(),
        53 => "the game or its Steam emulator aborted before the first frame; check GAMEID, the prefix and the repack notes"
            .to_string(),
        0xC000_0135 => "a required DLL is missing from the prefix; the prerequisite recipe may not have applied"
            .to_string(),
        0xC000_007B => "the executable image is invalid for this prefix; reset the prefix and try again"
            .to_string(),
        0xC000_0409 => "the game crashed (Windows fail-fast); check the launch log".to_string(),
        other => format!("the game exited with status 0x{other:08X}; check the launch log"),
    }
}

/// Evaluate every rule against the input and return the plan.
pub fn evaluate(input: &PlanInput, provider: &dyn RulesProvider) -> LaunchPlan {
    let mut plan = LaunchPlan {
        version: PLAN_VERSION,
        game_id: input.game_id,
        title: input.title.clone(),
        runtime: PlanRuntime::Umu,
        executable_known: input.executable.is_some(),
        proton: ProtonPreference::Pinned,
        tool_id: None,
        game_id_env: None,
        env: BTreeMap::new(),
        prefix: PrefixPolicy::PerGame,
        prerequisites: Vec::new(),
        wrappers: Wrappers::default(),
        display: input.display.clone(),
        first_frame_budget_secs: DEFAULT_FRAME_BUDGET_SECS,
        notes: Vec::new(),
        rules: Vec::new(),
        rules_version: provider.version(),
        source: PlanSource::Auto,
        sentence: String::new(),
        generated_at: String::new(),
    };

    let mut rules = provider.rules();
    // Stable sort: priority first, provider order for equal priorities.
    rules.sort_by_key(|rule| rule.priority);

    let mut wants_game_id = false;
    for rule in rules {
        if !rule.matches(input) {
            continue;
        }
        plan.rules.push(rule.id.clone());
        apply_patch(&mut plan, &rule.apply);
        wants_game_id |= rule.apply.game_id_from_input;
    }

    if wants_game_id {
        plan.game_id_env = input
            .known_game_id
            .clone()
            .filter(|value| !value.trim().is_empty());
        if let Some(game_id) = &plan.game_id_env {
            plan.env.insert("GAMEID".to_string(), game_id.clone());
        }
    }

    plan.wrappers.gamescope = plan.display.is_gamescope();
    plan.tool_id = resolve_tool(&plan.proton, &input.tools);
    plan.source = if is_manual_override(input) {
        PlanSource::Manual
    } else {
        PlanSource::Auto
    };
    plan.sentence = sentence(&plan);
    plan
}

fn apply_patch(plan: &mut LaunchPlan, patch: &PlanPatch) {
    if let Some(runtime) = patch.runtime {
        plan.runtime = runtime;
    }
    if let Some(proton) = &patch.proton {
        plan.proton = proton.clone();
    }
    for (key, value) in &patch.env {
        plan.env.insert(key.clone(), value.clone());
    }
    for id in &patch.prerequisites {
        let Some(recipe) = recipe_for(id) else {
            continue;
        };
        if !plan
            .prerequisites
            .iter()
            .any(|existing| existing.id == recipe.id)
        {
            plan.prerequisites.push(recipe);
        }
    }
    if let Some(gamemode) = patch.gamemode {
        plan.wrappers.gamemode = gamemode;
    }
    if let Some(mangohud) = patch.mangohud {
        plan.wrappers.mangohud = mangohud;
    }
    if let Some(budget) = patch.first_frame_budget_secs {
        plan.first_frame_budget_secs = budget;
    }
    if let Some(note) = &patch.note {
        plan.notes.push(note.clone());
    }
}

/// Pick the detected tool that best fits the Proton preference.
fn resolve_tool(preference: &ProtonPreference, tools: &[ToolCandidate]) -> Option<String> {
    let proton = || tools.iter().filter(|tool| tool.kind == "proton");
    match preference {
        ProtonPreference::Any | ProtonPreference::Steam => {
            // Steam leaves the choice to UMU/Proton; only an explicit vote is
            // recorded here.
            if *preference == ProtonPreference::Steam {
                proton()
                    .find(|tool| {
                        tool.name.to_ascii_lowercase().starts_with("proton")
                            && !tool.name.to_ascii_lowercase().contains("ge-proton")
                    })
                    .map(|tool| tool.id.clone())
            } else {
                None
            }
        }
        ProtonPreference::Pinned => proton()
            .find(|tool| tool.name.to_ascii_lowercase().contains("ge-proton"))
            .map(|tool| tool.id.clone()),
        ProtonPreference::Build(name) => proton()
            .find(|tool| tool.name.eq_ignore_ascii_case(name))
            .map(|tool| tool.id.clone()),
    }
}

/// One-sentence plan, e.g.
/// `Launching with UMU and GE-Proton, HDR off, GameMode on, GAMEID 1234.`
pub fn sentence(plan: &LaunchPlan) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !plan.executable_known {
        parts.push("Launching with the best available runtime".to_string());
    } else {
        match plan.runtime {
            PlanRuntime::Umu => {
                parts.push(format!("Launching with UMU and {}", plan.proton.label()));
            }
            PlanRuntime::Native => parts.push("Launching the native build".to_string()),
            PlanRuntime::Wine => parts.push("Launching with Wine".to_string()),
            PlanRuntime::LauncherUri => {
                parts.push("Opening the game through its own launcher".to_string())
            }
        }
    }
    if plan.display.is_gamescope() {
        match plan.display.resolution() {
            Some((width, height)) if plan.display.refresh > 0 => parts.push(format!(
                "{width}x{height} at {} Hz through Gamescope",
                plan.display.refresh
            )),
            Some((width, height)) => parts.push(format!("{width}x{height} through Gamescope")),
            None => parts.push("Gamescope".to_string()),
        }
    }
    parts.push(format!(
        "HDR {}",
        if plan.display.hdr { "on" } else { "off" }
    ));
    parts.push(format!(
        "GameMode {}",
        if plan.wrappers.gamemode { "on" } else { "off" }
    ));
    if plan.wrappers.mangohud {
        parts.push("MangoHud on".to_string());
    }
    if let Some(game_id) = &plan.game_id_env {
        parts.push(format!("GAMEID {game_id}"));
    }
    if !plan.prerequisites.is_empty() {
        let labels: Vec<&str> = plan
            .prerequisites
            .iter()
            .map(|prerequisite| prerequisite.label.as_str())
            .collect();
        parts.push(format!("installing {} first", labels.join(" + ")));
    }
    format!("{}.", parts.join(", "))
}

/* ---------------------------------------------------------------------- */
/* Override precedence                                                    */
/* ---------------------------------------------------------------------- */

/// True when the saved options are an explicit compatibility choice and the
/// rules must not touch the launch. Display, working directory and the Steam
/// app id are inputs, not compatibility overrides.
pub fn is_manual_override(input: &PlanInput) -> bool {
    input.manual_override
}

/// True when the stored launch options are an explicit compatibility choice.
pub fn options_are_manual(options: &LaunchOptions) -> bool {
    options.mode != LaunchMode::Auto
        || !options.tool_id.trim().is_empty()
        || !options.prefix_path.trim().is_empty()
        || !options.env.is_empty()
        || !options.winetricks_verbs.is_empty()
}

/// Merge the plan into the stored options. Per-field precedence: an explicit
/// value the user saved wins; the plan only fills the fields still on their
/// default. The stored row is never rewritten, so the plan re-evaluates on the
/// next launch when the machine changes.
pub fn apply_to_options(plan: &LaunchPlan, stored: &LaunchOptions) -> LaunchOptions {
    let mut options = stored.clone();
    if plan.executable_known {
        if options.mode == LaunchMode::Auto {
            options.mode = plan.launch_mode();
        }
        if options.tool_id.trim().is_empty() {
            options.tool_id = plan.tool_id.clone().unwrap_or_default();
        }
    }
    if options.display == DisplayProfile::default() {
        options.display = plan.display.clone();
    }
    for (key, value) in &plan.env {
        options
            .env
            .entry(key.clone())
            .or_insert_with(|| value.clone());
    }
    if plan.wrappers.mangohud {
        options
            .env
            .entry("MANGOHUD".to_string())
            .or_insert_with(|| "1".to_string());
    }
    // In-memory only: the Linux launch path wraps the entry point with
    // `gamemoderun`. Never persisted, so the recovery hash stays about the
    // user's own options. A manual plan is never applied.
    options.gamemode = plan.source == PlanSource::Auto && plan.wrappers.gamemode;
    options
}

/* ---------------------------------------------------------------------- */
/* Engine and GPU detection                                               */
/* ---------------------------------------------------------------------- */

const ENGINE_SCAN_DEPTH: usize = 3;
const ENGINE_SCAN_ENTRIES: usize = 400;

/// Prove the engine of an installed game from its marker files.
///
/// Bounded (3 levels, 400 entries) and read-only. Returns `None` when no
/// marker is recognised, never an error.
pub fn detect_engine(install_path: &Path) -> Option<Engine> {
    let mut queue = vec![(install_path.to_path_buf(), 0usize)];
    let mut entries = 0usize;
    let mut unity = false;
    let mut godot = false;
    let mut unreal = false;
    let mut uproject = false;
    while let Some((folder, depth)) = queue.pop() {
        if depth > ENGINE_SCAN_DEPTH || entries > ENGINE_SCAN_ENTRIES {
            break;
        }
        let Ok(read) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in read.flatten() {
            entries += 1;
            if entries > ENGINE_SCAN_ENTRIES {
                break;
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if name == "engine" || name == "binaries" {
                    unreal = true;
                }
                queue.push((path, depth + 1));
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name == "unityplayer.dll" {
                unity = true;
            } else if name.ends_with(".uproject") {
                uproject = true;
            } else if name.ends_with(".pck") || name == "project.godot" {
                godot = true;
            }
        }
    }
    if unreal || uproject {
        // A packaged build does not prove 4 vs 5; both markers mean Unreal.
        // The built-in rules treat the two the same, and a data provider can
        // refine the version from a title row.
        return Some(Engine::Unreal4);
    }
    if unity {
        return Some(Engine::Unity);
    }
    if godot {
        return Some(Engine::Godot);
    }
    None
}

/// Read the GPU vendor and kernel driver from DRM sysfs.
///
/// `root` is `/sys/class/drm` in production and a fixture tree in tests.
pub fn probe_gpu(root: &Path) -> GpuProfile {
    let Ok(entries) = std::fs::read_dir(root) else {
        return GpuProfile::default();
    };
    // `read_dir` order is filesystem-dependent; sort so the plan fingerprint is
    // stable across boots on a machine with several GPUs.
    let mut cards: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            (name.starts_with("card") && !name.contains('-')).then(|| entry.path())
        })
        .collect();
    cards.sort();
    for card in cards {
        let device = card.join("device");
        let vendor = std::fs::read_to_string(device.join("vendor"))
            .ok()
            .map(|text| text.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let vendor = match vendor.as_str() {
            "0x10de" => GpuVendor::Nvidia,
            "0x1002" => GpuVendor::Amd,
            "0x8086" => GpuVendor::Intel,
            "" => GpuVendor::Unknown,
            _ => GpuVendor::Other,
        };
        let uevent = std::fs::read_to_string(device.join("uevent")).unwrap_or_default();
        let driver_name = uevent
            .lines()
            .find_map(|line| line.strip_prefix("DRIVER="))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let driver = match driver_name.as_str() {
            "nvidia" => DriverFamily::NvidiaProprietary,
            "nouveau" => DriverFamily::Nouveau,
            "amdgpu" => DriverFamily::Amd,
            "i915" | "xe" => DriverFamily::Intel,
            _ => DriverFamily::Unknown,
        };
        if vendor == GpuVendor::Unknown && driver == DriverFamily::Unknown {
            continue;
        }
        return GpuProfile {
            vendor,
            driver,
            driver_name,
        };
    }
    GpuProfile::default()
}

/// Detect `gamemoderun` and MangoHud without running anything.
pub fn probe_wrappers(path_dirs: &[PathBuf]) -> (bool, bool) {
    let find = |name: &str| path_dirs.iter().any(|dir| dir.join(name).is_file());
    (find("gamemoderun"), find("mangohud"))
}

/// Absolute `gamemoderun` from `PATH`, when installed.
pub fn gamemoderun_path() -> Option<PathBuf> {
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    dirs.into_iter()
        .map(|dir| dir.join("gamemoderun"))
        .find(|path| path.is_file())
}

/// Put `gamemoderun` in front of the entry point, when the plan and the host
/// allow it. `gamemoderun` changes the environment and execs its arguments;
/// no `--` separator is used (the script does not parse one).
pub fn wrap_entry(
    program: PathBuf,
    args: Vec<String>,
    wrappers: &Wrappers,
    gamemoderun: Option<PathBuf>,
) -> (PathBuf, Vec<String>) {
    let Some(gamemoderun) = gamemoderun.filter(|_| wrappers.gamemode) else {
        return (program, args);
    };
    let mut wrapped = Vec::with_capacity(args.len() + 1);
    wrapped.push(program.to_string_lossy().to_string());
    wrapped.extend(args);
    (gamemoderun, wrapped)
}

/* ---------------------------------------------------------------------- */
/* Prerequisites                                                          */
/* ---------------------------------------------------------------------- */

const MARKER_DIR: &str = ".ludex";
const MARKER_FILE: &str = "prerequisites.json";

/// What has already been applied to one prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrerequisiteMarker {
    pub fingerprint: String,
    pub applied: Vec<String>,
    pub failed: Vec<String>,
    pub updated_at: String,
}

fn marker_path(prefix: &Path) -> PathBuf {
    prefix.join(MARKER_DIR).join(MARKER_FILE)
}

pub fn read_marker(prefix: &Path) -> Option<PrerequisiteMarker> {
    let text = std::fs::read_to_string(marker_path(prefix)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_marker(prefix: &Path, marker: &PrerequisiteMarker) -> Result<(), String> {
    let path = marker_path(prefix);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(marker)
        .map_err(|error| format!("Could not encode the prerequisite marker: {error}"))?;
    std::fs::write(&path, text)
        .map_err(|error| format!("Could not write {}: {error}", path.display()))
}

/// Fingerprint of a recipe set, so changing the recipe re-runs it.
pub fn recipe_fingerprint(prerequisites: &[Prerequisite]) -> String {
    let ids: Vec<&str> = prerequisites
        .iter()
        .map(|prerequisite| prerequisite.id.as_str())
        .collect();
    let digest = Sha256::digest(ids.join(",").as_bytes());
    hex::encode(&digest[..8])
}

/// Recipes still missing from the prefix.
///
/// A marker written for a different recipe (a rules update) restarts the whole
/// recipe: a partially applied prefix is never assumed complete.
pub fn pending_prerequisites(
    plan: &LaunchPlan,
    marker: Option<&PrerequisiteMarker>,
) -> Vec<Prerequisite> {
    if plan.prerequisites.is_empty() {
        return Vec::new();
    }
    let fingerprint = recipe_fingerprint(&plan.prerequisites);
    let Some(marker) = marker else {
        return plan.prerequisites.clone();
    };
    if marker.fingerprint != fingerprint {
        return plan.prerequisites.clone();
    }
    plan.prerequisites
        .iter()
        .filter(|prerequisite| {
            !marker
                .applied
                .iter()
                .any(|applied| applied == &prerequisite.id)
        })
        .cloned()
        .collect()
}

/* ---------------------------------------------------------------------- */
/* Persistence                                                            */
/* ---------------------------------------------------------------------- */

/// `<appDataDir>/launch-plans`.
pub fn store_dir(app_data: &Path) -> PathBuf {
    app_data.join("launch-plans")
}

pub fn plan_path(app_data: &Path, game_id: i64) -> PathBuf {
    store_dir(app_data).join(format!("{game_id}.json"))
}

/// The stored plan and the fingerprint of the input it was built from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRecord {
    pub version: u32,
    pub fingerprint: String,
    pub saved_at: String,
    pub plan: LaunchPlan,
    /// Last failed session, kept until a session runs past its budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<PlanFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<String>,
}

pub fn load_record(app_data: &Path, game_id: i64) -> Option<PlanRecord> {
    let text = std::fs::read_to_string(plan_path(app_data, game_id)).ok()?;
    let record: PlanRecord = serde_json::from_str(&text).ok()?;
    (record.version == PLAN_VERSION).then_some(record)
}

pub fn save_record(app_data: &Path, record: &PlanRecord) -> Result<PathBuf, String> {
    let path = plan_path(app_data, record.plan.game_id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(record)
        .map_err(|error| format!("Could not encode the launch plan: {error}"))?;
    std::fs::write(&path, text)
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    Ok(path)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn remove_record(app_data: &Path, game_id: i64) {
    let _ = std::fs::remove_file(plan_path(app_data, game_id));
}

/// Append the plan section to a launch log that already has its #298 header.
///
/// Called after `prepare_launch` opened and flushed the header, before the
/// process writes its own output.
pub fn append_log(path: &Path, plan: &LaunchPlan) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(b"--- launch plan ---\n")?;
    for line in log_lines(plan) {
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
    }
    file.flush()
}

/// Portable export of one plan. Stable JSON, no machine paths.
pub fn export_plan(plan: &LaunchPlan) -> Result<String, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Export<'a> {
        kind: &'static str,
        version: u32,
        plan: &'a LaunchPlan,
    }
    serde_json::to_string_pretty(&Export {
        kind: "ludex-launch-plan",
        version: PLAN_VERSION,
        plan,
    })
    .map_err(|error| format!("Could not export the launch plan: {error}"))
}

/// Record a failed auto launch next to the stored plan (#297 → #298).
pub fn record_failure(
    app_data: &Path,
    game_id: i64,
    failure: &PlanFailure,
    now: &str,
) -> Result<(), String> {
    let Some(mut record) = load_record(app_data, game_id) else {
        return Ok(());
    };
    record.last_failure = Some(failure.clone());
    record.last_failure_at = Some(now.to_string());
    save_record(app_data, &record).map(|_| ())
}

/// A session ran past its budget: the plan is confirmed working.
pub fn clear_failure(app_data: &Path, game_id: i64) -> Result<(), String> {
    let Some(mut record) = load_record(app_data, game_id) else {
        return Ok(());
    };
    if record.last_failure.is_none() && record.last_failure_at.is_none() {
        return Ok(());
    }
    record.last_failure = None;
    record.last_failure_at = None;
    save_record(app_data, &record).map(|_| ())
}

/// Rules and inputs that produced a plan, for the launch log.
pub fn log_lines(plan: &LaunchPlan) -> Vec<String> {
    let mut lines = vec![
        format!("plan: {}", plan.sentence),
        format!(
            "plan runtime: {} · proton: {} · prefix: {} · budget: {}s",
            plan.runtime.as_str(),
            plan.proton.label(),
            plan.prefix.as_str(),
            plan.first_frame_budget_secs
        ),
        format!(
            "plan rules: {} ({})",
            plan.rules.join(", "),
            plan.rules_version
        ),
        format!(
            "plan source: {}",
            match plan.source {
                PlanSource::Auto => "auto",
                PlanSource::Manual => "manual override (plan not applied)",
            }
        ),
    ];
    if !plan.prerequisites.is_empty() {
        lines.push(format!(
            "plan prerequisites: {}",
            plan.prerequisites
                .iter()
                .map(|prerequisite| prerequisite.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for note in &plan.notes {
        lines.push(format!("plan note: {note}"));
    }
    lines
}

/* ---------------------------------------------------------------------- */
/* Application layer: input from the local index                          */
/* ---------------------------------------------------------------------- */

use crate::compat::CompatScan;
use crate::db;
use crate::state::AppState;
use tauri::Manager;

/// Build the rules input for one installed game.
pub fn build_input(
    installed: &db::InstalledGame,
    options: &LaunchOptions,
    scan: &CompatScan,
) -> PlanInput {
    let install_path = crate::paths::normalize_native_path(&PathBuf::from(&installed.install_path));
    let executable = installed.executable.clone();
    let exe_path = executable
        .as_deref()
        .map(|raw| crate::paths::normalize_native_path(&install_path.join(raw)));
    let exe_machine = exe_path
        .as_deref()
        .filter(|path| path.starts_with(&install_path))
        .and_then(crate::runtime::plan::pe_machine);
    let windows_executable = exe_machine.is_some()
        || executable
            .as_deref()
            .map(|value| value.to_ascii_lowercase().ends_with(".exe"))
            .unwrap_or(false);
    let archive_origin = if installed
        .launch_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty())
        .is_some()
    {
        ArchiveOrigin::Launcher
    } else if installed.source == db::SOURCE_ADOPTED {
        ArchiveOrigin::Adopted
    } else {
        ArchiveOrigin::Ludex
    };
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    let (gamemode, mangohud) = probe_wrappers(&path_dirs);
    PlanInput {
        game_id: installed.game_id,
        title: installed.title.clone(),
        executable,
        windows_executable,
        exe_machine,
        engine: detect_engine(&install_path),
        known_game_id: options.steam_app_id.map(|id| id.to_string()),
        archive_origin,
        gpu: probe_gpu(Path::new(crate::display_caps::DEFAULT_DRM_ROOT)),
        gamemode,
        mangohud,
        winetricks: scan.winetricks.is_some(),
        declared_prerequisites: crate::runtime::plan::declared_prerequisites(&install_path),
        display: options.display.clone(),
        manual_override: options_are_manual(options),
        host_arch: std::env::consts::ARCH.to_string(),
        tools: scan
            .tools
            .iter()
            .map(|tool| ToolCandidate {
                id: tool.id.clone(),
                name: tool.name.clone(),
                kind: tool.kind.as_str().to_string(),
                proton_path: tool.proton_path.clone(),
            })
            .collect(),
    }
}

/// One evaluated plan plus what the view needs to explain it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlanView {
    pub plan: LaunchPlan,
    /// Where the stored plan lives (JSON, exportable).
    pub path: String,
    pub stored: bool,
    /// The stored plan was built from different inputs; refresh re-evaluates.
    pub stale: bool,
    /// Prerequisites are being applied for this game right now.
    pub running: bool,
    pub last_failure: Option<PlanFailure>,
    pub last_failure_at: Option<String>,
}

fn plan_view(app_data: &Path, input: &PlanInput, now: &str) -> (LaunchPlan, LaunchPlanView) {
    let record = load_record(app_data, input.game_id);
    let fresh = evaluate(input, &BuiltinRules);
    let stale = record
        .as_ref()
        .map(|record| record.fingerprint != input.fingerprint())
        .unwrap_or(true);
    let plan = match &record {
        Some(record) if !stale => record.plan.clone(),
        _ => {
            let mut plan = fresh;
            plan.generated_at = now.to_string();
            plan
        }
    };
    let view = LaunchPlanView {
        path: plan_path(app_data, input.game_id)
            .to_string_lossy()
            .to_string(),
        stored: record.is_some(),
        stale,
        running: prerequisite_running(input.game_id),
        last_failure: record
            .as_ref()
            .and_then(|record| record.last_failure.clone()),
        last_failure_at: record.and_then(|record| record.last_failure_at),
        plan: plan.clone(),
    };
    (plan, view)
}

/// Read (or evaluate) the plan for a game without writing anything.
pub fn view_for_game(app_data: &Path, input: &PlanInput, now: &str) -> LaunchPlanView {
    plan_view(app_data, input, now).1
}

/// Evaluate and store the plan, so the next launch reuses it verbatim.
pub fn refresh_for_game(
    app_data: &Path,
    input: &PlanInput,
    now: &str,
) -> Result<LaunchPlanView, String> {
    let mut plan = evaluate(input, &BuiltinRules);
    plan.generated_at = now.to_string();
    let record = PlanRecord {
        version: PLAN_VERSION,
        fingerprint: input.fingerprint(),
        saved_at: now.to_string(),
        plan: plan.clone(),
        last_failure: None,
        last_failure_at: None,
    };
    save_record(app_data, &record)?;
    Ok(plan_view(app_data, input, now).1)
}

/* ---------------------------------------------------------------------- */
/* Prerequisites: progress and cancel                                     */
/* ---------------------------------------------------------------------- */

/// Progress emitted on `launch://plan-progress`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanProgress {
    pub game_id: i64,
    /// `prerequisites`, `done`, `failed`.
    pub phase: String,
    pub step_id: Option<String>,
    pub index: usize,
    pub total: usize,
    pub percent: Option<f64>,
    /// Human text, never a winetricks verb.
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrerequisiteOutcome {
    pub applied: Vec<String>,
    pub failed: Vec<String>,
    /// Recipe ids skipped because winetricks is not installed.
    pub skipped: Vec<String>,
}

/// Cancellation tokens for in-flight prerequisite runs, keyed by game id.
///
/// The maps only ever hold runs started by this process; a restart loses a
/// token, which is correct because the child died with the app.
fn runs() -> &'static std::sync::Mutex<BTreeMap<i64, tokio_util::sync::CancellationToken>> {
    static RUNS: std::sync::OnceLock<
        std::sync::Mutex<BTreeMap<i64, tokio_util::sync::CancellationToken>>,
    > = std::sync::OnceLock::new();
    RUNS.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

pub fn prerequisite_running(game_id: i64) -> bool {
    runs()
        .lock()
        .map(|runs| runs.contains_key(&game_id))
        .unwrap_or(false)
}

/// Cancel the prerequisite run of one game. True when a run was cancelled.
pub fn cancel_prerequisites(game_id: i64) -> bool {
    let Ok(runs) = runs().lock() else {
        return false;
    };
    match runs.get(&game_id) {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

struct PrerequisiteGuard {
    game_id: i64,
}

impl Drop for PrerequisiteGuard {
    fn drop(&mut self) {
        if let Ok(mut runs) = runs().lock() {
            runs.remove(&self.game_id);
        }
    }
}

/// Apply the recipes the plan asks for that the prefix does not have yet.
///
/// Crosses process boundaries, so it is async; the pure gate is
/// [`pending_prerequisites`]. A missing `winetricks` or a failing verb never
/// blocks the game: it is reported and the launch goes on.
#[allow(clippy::too_many_arguments)]
pub async fn apply_prerequisites(
    app: &tauri::AppHandle,
    game_id: i64,
    plan: &LaunchPlan,
    prefix: &Path,
    winetricks: Option<&str>,
    tools: &[crate::compat::CompatTool],
    steam_client: Option<&str>,
    now: &str,
) -> Result<PrerequisiteOutcome, String> {
    use tauri::Emitter;

    let marker = read_marker(prefix);
    let pending = pending_prerequisites(plan, marker.as_ref());
    let mut outcome = PrerequisiteOutcome::default();
    if pending.is_empty() {
        return Ok(outcome);
    }

    let emit = |phase: &str, index: usize, total: usize, step: Option<&str>, message: String| {
        let percent = (total > 0).then(|| index as f64 / total as f64 * 100.0);
        let _ = app.emit(
            "launch://plan-progress",
            PlanProgress {
                game_id,
                phase: phase.to_string(),
                step_id: step.map(str::to_string),
                index,
                total,
                percent,
                message,
            },
        );
    };

    let Some(winetricks) = winetricks.map(str::trim).filter(|value| !value.is_empty()) else {
        outcome.skipped = pending
            .iter()
            .map(|prerequisite| prerequisite.id.clone())
            .collect();
        emit(
            "failed",
            0,
            pending.len(),
            None,
            "winetricks is not installed; the prerequisite recipe was skipped".to_string(),
        );
        return Ok(outcome);
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    {
        let mut runs = runs()
            .lock()
            .map_err(|_| "The prerequisite state is unavailable; restart Ludex.".to_string())?;
        if runs.contains_key(&game_id) {
            return Err("The prerequisites are already being applied.".to_string());
        }
        runs.insert(game_id, cancel.clone());
    }
    let _guard = PrerequisiteGuard { game_id };

    std::fs::create_dir_all(prefix).map_err(|error| {
        format!(
            "Could not create the Wine prefix {}: {error}",
            prefix.display()
        )
    })?;

    let total: usize = pending.iter().map(|step| step.verbs.len()).sum();
    let selection = crate::linux_gaming::select_tool(tools, LaunchMode::Umu, "")
        .ok()
        .map(|mut selection| {
            selection.client_install_path = steam_client.map(PathBuf::from);
            selection
        });
    let mut done = 0usize;
    for step in &pending {
        if cancel.is_cancelled() {
            emit(
                "failed",
                done,
                total,
                Some(&step.id),
                "Prerequisites cancelled".to_string(),
            );
            return Err("Launch cancelled before the prerequisites finished.".to_string());
        }
        for verb in &step.verbs {
            if cancel.is_cancelled() {
                emit(
                    "failed",
                    done,
                    total,
                    Some(&step.id),
                    "Prerequisites cancelled".to_string(),
                );
                return Err("Launch cancelled before the prerequisites finished.".to_string());
            }
            emit(
                "prerequisites",
                done,
                total,
                Some(&step.id),
                format!("Installing {} ({}/{})", step.label, done + 1, total),
            );
            let command = crate::linux_gaming::build_winetricks(
                Path::new(winetricks),
                std::slice::from_ref(verb),
                prefix,
                selection.as_ref(),
            )?;
            let report = tokio::task::spawn_blocking(move || {
                crate::prefix::run_winetricks(&command, crate::prefix::WINETRICKS_TIMEOUT)
            })
            .await
            .map_err(|error| format!("The prerequisite task failed: {error}"))??;
            done += 1;
            if report.success {
                if !outcome.applied.contains(&step.id) {
                    outcome.applied.push(step.id.clone());
                }
                outcome.failed.retain(|id| id != &step.id);
            } else if !outcome.failed.contains(&step.id) {
                outcome.failed.push(step.id.clone());
            }
        }
        // Persist progress after every recipe so a cancel or a crash does not
        // repeat a completed verb set.
        let mut updated = read_marker(prefix).unwrap_or_default();
        updated.fingerprint = recipe_fingerprint(&plan.prerequisites);
        updated.applied = merge_ids(&updated.applied, &outcome.applied);
        updated.failed = outcome.failed.clone();
        updated.updated_at = now.to_string();
        let _ = write_marker(prefix, &updated);
    }

    emit(
        "done",
        total,
        total,
        None,
        if outcome.failed.is_empty() {
            "Prerequisites ready".to_string()
        } else {
            format!(
                "{} prerequisite(s) could not be installed; the launch continues",
                outcome.failed.len()
            )
        },
    );
    Ok(outcome)
}

fn merge_ids(existing: &[String], added: &[String]) -> Vec<String> {
    let mut merged: Vec<String> = existing.to_vec();
    for id in added {
        if !merged.contains(id) {
            merged.push(id.clone());
        }
    }
    merged
}

/* ---------------------------------------------------------------------- */
/* Launch integration                                                     */
/* ---------------------------------------------------------------------- */

/// What the launch path should use for this session.
#[derive(Debug, Clone)]
pub struct AutoLaunch {
    pub plan: LaunchPlan,
    /// Options after the plan filled its defaults. An explicit saved value
    /// always won.
    pub options: LaunchOptions,
    pub prerequisites: PrerequisiteOutcome,
    /// True when the stored plan matched the current inputs and was reused.
    pub reused: bool,
}

/// Evaluate (or reuse) the plan, run the prerequisite recipe and return the
/// options the process must be started with.
///
/// Called from [`crate::launch::launch`] on Linux only. A manual override is
/// returned untouched: the plan is recorded for the log, never applied.
pub async fn prepare_for_launch(
    app: &tauri::AppHandle,
    installed: &db::InstalledGame,
    stored: &LaunchOptions,
    prefix: &Path,
    scan: &CompatScan,
    now: &str,
) -> Result<AutoLaunch, String> {
    let state = app.state::<AppState>();
    let input = build_input(installed, stored, scan);
    let record = load_record(&state.app_data, input.game_id);
    let reused = record
        .as_ref()
        .map(|record| record.fingerprint == input.fingerprint())
        .unwrap_or(false);
    let mut plan = match &record {
        Some(record) if reused => record.plan.clone(),
        _ => {
            let mut plan = evaluate(&input, &BuiltinRules);
            plan.generated_at = now.to_string();
            plan
        }
    };
    // Re-evaluate the source flag even when the stored plan is reused: the
    // options may have been reset to Auto since it was saved.
    plan.source = if input.manual_override {
        PlanSource::Manual
    } else {
        PlanSource::Auto
    };
    plan.sentence = sentence(&plan);

    let mut outcome = PrerequisiteOutcome::default();
    if plan.source == PlanSource::Auto {
        outcome = apply_prerequisites(
            app,
            input.game_id,
            &plan,
            prefix,
            scan.winetricks.as_deref(),
            &scan.tools,
            scan.steam_client_path.as_deref(),
            now,
        )
        .await?;
    }

    let options = if plan.source == PlanSource::Auto {
        apply_to_options(&plan, stored)
    } else {
        stored.clone()
    };

    let mut next = PlanRecord {
        version: PLAN_VERSION,
        fingerprint: input.fingerprint(),
        saved_at: now.to_string(),
        plan: plan.clone(),
        last_failure: record
            .as_ref()
            .and_then(|record| record.last_failure.clone()),
        last_failure_at: record
            .as_ref()
            .and_then(|record| record.last_failure_at.clone()),
    };
    // A manual override is not the rules' plan: keep it out of the stored
    // record so removing the override brings the plan back unchanged.
    if plan.source == PlanSource::Auto {
        save_record(&state.app_data, &next)?;
    } else {
        next.last_failure = None;
    }

    Ok(AutoLaunch {
        plan,
        options,
        prerequisites: outcome,
        reused,
    })
}

/* ---------------------------------------------------------------------- */
/* Tauri commands                                                         */
/* ---------------------------------------------------------------------- */

fn installed_for(state: &AppState, game_id: i64) -> Result<db::InstalledGame, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::get_installed(&connection, game_id)?
        .ok_or_else(|| format!("Game {game_id} is not installed on this device."))
}

fn options_for(state: &AppState, game_id: i64) -> Result<LaunchOptions, String> {
    let connection = state
        .db
        .lock()
        .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
    db::get_launch_options(&connection, game_id)
}

fn view_sync(
    app: &tauri::AppHandle,
    game_id: i64,
    refresh: bool,
) -> Result<LaunchPlanView, String> {
    let state = app.state::<AppState>();
    let installed = installed_for(&state, game_id)?;
    let options = options_for(&state, game_id)?;
    let scan = crate::compat::scan(&state.settings_snapshot());
    let input = build_input(&installed, &options, &scan);
    let now = crate::util::now_rfc3339();
    if refresh {
        refresh_for_game(&state.app_data, &input, &now)
    } else {
        Ok(view_for_game(&state.app_data, &input, &now))
    }
}

/// The plan for one game, evaluated from the stored plan when it is current.
#[tauri::command]
pub async fn launch_plan_get(
    app: tauri::AppHandle,
    game_id: i64,
) -> Result<LaunchPlanView, String> {
    tauri::async_runtime::spawn_blocking(move || view_sync(&app, game_id, false))
        .await
        .map_err(|error| format!("The launch plan could not be read: {error}"))?
}

/// Re-evaluate and store the plan. The next launch reuses it.
#[tauri::command]
pub async fn launch_plan_refresh(
    app: tauri::AppHandle,
    game_id: i64,
) -> Result<LaunchPlanView, String> {
    tauri::async_runtime::spawn_blocking(move || view_sync(&app, game_id, true))
        .await
        .map_err(|error| format!("The launch plan could not be refreshed: {error}"))?
}

/// Portable JSON export of the plan (or of a fresh evaluation when none is
/// stored).
#[tauri::command]
pub async fn launch_plan_export(app: tauri::AppHandle, game_id: i64) -> Result<String, String> {
    let view = launch_plan_get(app, game_id).await?;
    export_plan(&view.plan)
}

/// Cancel the prerequisite run of one game, if one is in flight.
#[tauri::command]
pub fn launch_plan_cancel(game_id: i64) -> bool {
    cancel_prerequisites(game_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_gaming::{DisplayMode, DisplayProfile};

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-launch-plan-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, bytes: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn tool(name: &str) -> ToolCandidate {
        ToolCandidate {
            id: format!("proton:/runtimes/{name}"),
            name: name.to_string(),
            kind: "proton".to_string(),
            proton_path: Some(format!("/runtimes/{name}")),
        }
    }

    fn base_input() -> PlanInput {
        PlanInput {
            game_id: 7,
            title: "Hades".to_string(),
            executable: Some("Hades.exe".to_string()),
            windows_executable: true,
            exe_machine: Some("x86_64".to_string()),
            engine: None,
            known_game_id: Some("1145360".to_string()),
            archive_origin: ArchiveOrigin::Ludex,
            gpu: GpuProfile {
                vendor: GpuVendor::Amd,
                driver: DriverFamily::Amd,
                driver_name: "amdgpu".to_string(),
            },
            gamemode: true,
            mangohud: false,
            winetricks: false,
            declared_prerequisites: Vec::new(),
            display: DisplayProfile::default(),
            manual_override: false,
            host_arch: "x86_64".to_string(),
            tools: vec![tool("GE-Proton11-12")],
        }
    }

    fn options() -> LaunchOptions {
        LaunchOptions::default()
    }

    #[test]
    fn a_windows_game_gets_umu_ge_proton_gamemode_and_the_steam_game_id() {
        let plan = evaluate(&base_input(), &BuiltinRules);
        assert_eq!(plan.runtime, PlanRuntime::Umu);
        assert_eq!(plan.proton, ProtonPreference::Pinned);
        assert_eq!(
            plan.tool_id.as_deref(),
            Some("proton:/runtimes/GE-Proton11-12")
        );
        assert_eq!(plan.game_id_env.as_deref(), Some("1145360"));
        assert_eq!(plan.env.get("GAMEID").map(String::as_str), Some("1145360"));
        assert!(plan.wrappers.gamemode);
        assert_eq!(plan.prefix, PrefixPolicy::PerGame);
        assert_eq!(
            plan.sentence,
            "Launching with UMU and GE-Proton, HDR off, GameMode on, GAMEID 1145360."
        );
        assert_eq!(plan.source, PlanSource::Auto);
        assert_eq!(
            plan.rules,
            vec!["windows-exe-umu", "steam-game-id", "gamemode-if-installed"]
        );
    }

    #[test]
    fn a_native_linux_game_runs_without_a_compatibility_layer() {
        let mut input = base_input();
        input.executable = Some("Hades.sh".to_string());
        input.windows_executable = false;
        input.exe_machine = None;
        input.known_game_id = None;
        let plan = evaluate(&input, &BuiltinRules);
        assert_eq!(plan.runtime, PlanRuntime::Native);
        assert_eq!(plan.rules, vec!["native-build"]);
        assert!(plan.game_id_env.is_none());
        assert_eq!(
            plan.sentence,
            "Launching the native build, HDR off, GameMode off."
        );
        assert_eq!(plan.launch_mode(), LaunchMode::Native);
    }

    #[test]
    fn an_unknown_executable_leaves_the_runtime_to_the_auto_rule() {
        let mut input = base_input();
        input.executable = None;
        input.exe_machine = None;
        input.windows_executable = false;
        let plan = evaluate(&input, &BuiltinRules);
        assert!(!plan.executable_known);
        assert!(plan
            .sentence
            .starts_with("Launching with the best available runtime"));
        let applied = apply_to_options(&plan, &options());
        assert_eq!(applied.mode, LaunchMode::Auto);
        assert!(applied.tool_id.is_empty());
    }

    #[test]
    fn an_imported_launcher_entry_keeps_its_uri() {
        let mut input = base_input();
        input.archive_origin = ArchiveOrigin::Launcher;
        let plan = evaluate(&input, &BuiltinRules);
        assert_eq!(plan.runtime, PlanRuntime::LauncherUri);
        assert!(plan.rules.contains(&"launcher-uri".to_string()));
        assert_eq!(plan.launch_mode(), LaunchMode::Auto);
    }

    #[test]
    fn engine_markers_are_detected_from_the_install_folder() {
        let root = temp_root("engine");
        write(&root.join("UnityPlayer.dll"), "MZ");
        assert_eq!(detect_engine(&root), Some(Engine::Unity));

        let unreal = temp_root("unreal");
        std::fs::create_dir_all(unreal.join("Engine/Binaries/Win64")).unwrap();
        write(&unreal.join("Game/Binaries/Win64/Game.exe"), "MZ");
        assert_eq!(detect_engine(&unreal), Some(Engine::Unreal4));

        let godot = temp_root("godot");
        write(&godot.join("game.pck"), "GDPC");
        assert_eq!(detect_engine(&godot), Some(Engine::Godot));

        let unknown = temp_root("unknown");
        write(&unknown.join("readme.txt"), "hello");
        assert_eq!(detect_engine(&unknown), None);
    }

    #[test]
    fn the_engine_rule_raises_the_first_frame_budget() {
        let mut input = base_input();
        input.engine = Some(Engine::Unity);
        let plan = evaluate(&input, &BuiltinRules);
        assert_eq!(plan.first_frame_budget_secs, 15);
        assert!(plan.rules.contains(&"engine-shader-budget".to_string()));
        assert_eq!(plan.notes.len(), 1);
        // The budget is not part of the sentence: it is a guard, not a promise.
        assert!(!plan.sentence.contains("15"));

        input.engine = Some(Engine::Godot);
        let plan = evaluate(&input, &BuiltinRules);
        assert_eq!(plan.first_frame_budget_secs, DEFAULT_FRAME_BUDGET_SECS);
    }

    #[test]
    fn gpu_sysfs_fixture_produces_the_vendor_and_driver_profile() {
        let root = temp_root("gpu");
        write(&root.join("card1/device/vendor"), "0x10de\n");
        write(
            &root.join("card1/device/uevent"),
            "DRIVER=nvidia\nPCI_CLASS=30000\n",
        );
        let profile = probe_gpu(&root);
        assert_eq!(profile.vendor, GpuVendor::Nvidia);
        assert_eq!(profile.driver, DriverFamily::NvidiaProprietary);

        let nouveau = temp_root("nouveau");
        write(&nouveau.join("card0/device/vendor"), "0x10de\n");
        write(&nouveau.join("card0/device/uevent"), "DRIVER=nouveau\n");
        let mut input = base_input();
        input.gpu = probe_gpu(&nouveau);
        let plan = evaluate(&input, &BuiltinRules);
        assert!(plan.rules.contains(&"nouveau-no-vulkan".to_string()));
        assert!(plan.notes.iter().any(|note| note.contains("Nouveau")));

        input.gpu = profile;
        let plan = evaluate(&input, &BuiltinRules);
        assert!(!plan.rules.contains(&"nouveau-no-vulkan".to_string()));
    }

    #[test]
    fn an_amd_game_has_no_extra_warning() {
        let plan = evaluate(&base_input(), &BuiltinRules);
        assert!(plan.notes.is_empty());
    }

    #[test]
    fn a_gamescope_profile_puts_the_resolution_in_the_sentence() {
        let mut input = base_input();
        input.display = DisplayProfile {
            mode: DisplayMode::Gamescope,
            width: 1920,
            height: 1080,
            refresh: 60,
            filter: "fsr".to_string(),
            adaptive_sync: true,
            hdr: false,
        };
        let plan = evaluate(&input, &BuiltinRules);
        assert!(plan.wrappers.gamescope);
        assert!(plan
            .sentence
            .contains("1920x1080 at 60 Hz through Gamescope"));
        assert_eq!(plan.display, input.display);
    }

    #[test]
    fn a_declared_redistributable_becomes_a_named_prerequisite() {
        let mut input = base_input();
        input.declared_prerequisites = vec!["vcrun".to_string(), "directx".to_string()];
        let plan = evaluate(&input, &BuiltinRules);
        let ids: Vec<&str> = plan
            .prerequisites
            .iter()
            .map(|prerequisite| prerequisite.id.as_str())
            .collect();
        assert_eq!(ids, vec!["vcrun", "directx"]);
        assert_eq!(
            plan.sentence,
            "Launching with UMU and GE-Proton, HDR off, GameMode on, GAMEID 1145360, \
             installing Visual C++ 2015–2022 runtime + DirectX 9 runtime first."
        );
        // The verb is never part of the human sentence.
        assert!(!plan.sentence.contains("vcrun2019"));
    }

    #[test]
    fn a_physx_declaration_has_no_automatic_recipe() {
        let mut input = base_input();
        input.declared_prerequisites = vec!["physx".to_string()];
        let plan = evaluate(&input, &BuiltinRules);
        assert!(plan.prerequisites.is_empty());
    }

    #[test]
    fn a_custom_rules_provider_can_replace_the_plan() {
        struct CuratedRules;
        impl RulesProvider for CuratedRules {
            fn rules(&self) -> Vec<Rule> {
                vec![Rule {
                    id: "curated-hades".to_string(),
                    priority: 0,
                    summary: "Curated rule for one title.".to_string(),
                    when: Match {
                        known_game_ids: vec!["1145360".to_string()],
                        ..Match::default()
                    },
                    apply: PlanPatch {
                        proton: Some(ProtonPreference::Build("GE-Proton11-12".to_string())),
                        env: vec![("DXVK_HUD".to_string(), "fps".to_string())],
                        ..PlanPatch::default()
                    },
                }]
            }

            fn version(&self) -> String {
                "test-1".to_string()
            }
        }

        let plan = evaluate(&base_input(), &CuratedRules);
        assert_eq!(plan.rules_version, "test-1");
        assert_eq!(plan.rules, vec!["curated-hades"]);
        assert_eq!(
            plan.proton,
            ProtonPreference::Build("GE-Proton11-12".to_string())
        );
        assert_eq!(plan.env.get("DXVK_HUD").map(String::as_str), Some("fps"));
        // Only the provider's rule ran: no GAMEID, no GameMode.
        assert!(plan.game_id_env.is_none());
        assert!(!plan.wrappers.gamemode);
    }

    #[test]
    fn manual_options_win_over_every_plan_field() {
        let manual = LaunchOptions {
            mode: LaunchMode::Proton,
            tool_id: "proton:/steam/Proton - Experimental".to_string(),
            prefix_path: "/data/prefixes/hades".to_string(),
            env: BTreeMap::from([("GAMEID".to_string(), "manual".to_string())]),
            winetricks_verbs: vec!["corefonts".to_string()],
            ..LaunchOptions::default()
        };
        let mut input = base_input();
        input.manual_override = true;
        let plan = evaluate(&input, &BuiltinRules);
        assert_eq!(plan.source, PlanSource::Manual);

        let applied = apply_to_options(&plan, &manual);
        assert_eq!(applied.mode, LaunchMode::Proton);
        assert!(!applied.gamemode, "a manual plan is never applied");
        assert_eq!(applied.tool_id, manual.tool_id);
        assert_eq!(applied.prefix_path, manual.prefix_path);
        assert_eq!(
            applied.env.get("GAMEID").map(String::as_str),
            Some("manual")
        );
        assert_eq!(applied.winetricks_verbs, manual.winetricks_verbs);
    }

    #[test]
    fn the_plan_only_fills_fields_still_on_their_default() {
        let plan = evaluate(&base_input(), &BuiltinRules);
        let applied = apply_to_options(&plan, &options());
        assert_eq!(applied.mode, LaunchMode::Umu);
        assert_eq!(applied.tool_id, plan.tool_id.clone().unwrap());
        assert_eq!(
            applied.env.get("GAMEID").map(String::as_str),
            Some("1145360")
        );
        assert_eq!(applied.display, DisplayProfile::default());

        let display = DisplayProfile {
            mode: DisplayMode::Gamescope,
            width: 1280,
            height: 720,
            ..DisplayProfile::default()
        };
        let stored = LaunchOptions {
            display,
            ..LaunchOptions::default()
        };
        let applied = apply_to_options(&plan, &stored);
        assert_eq!(applied.display.mode, DisplayMode::Gamescope);
        assert_eq!(applied.display.width, 1280);
    }

    #[test]
    fn mangohud_is_enabled_without_overwriting_a_manual_value() {
        let mut input = base_input();
        input.mangohud = true;
        struct HudRules;
        impl RulesProvider for HudRules {
            fn rules(&self) -> Vec<Rule> {
                vec![Rule {
                    id: "hud".to_string(),
                    priority: 0,
                    summary: String::new(),
                    when: Match::default(),
                    apply: PlanPatch {
                        mangohud: Some(true),
                        ..PlanPatch::default()
                    },
                }]
            }
        }
        let plan = evaluate(&input, &HudRules);
        let applied = apply_to_options(&plan, &options());
        assert_eq!(applied.env.get("MANGOHUD").map(String::as_str), Some("1"));

        let stored = LaunchOptions {
            env: BTreeMap::from([("MANGOHUD".to_string(), "0".to_string())]),
            ..LaunchOptions::default()
        };
        let applied = apply_to_options(&plan, &stored);
        assert_eq!(applied.env.get("MANGOHUD").map(String::as_str), Some("0"));
    }

    #[test]
    fn a_plan_round_trips_through_the_store_and_goes_stale_with_its_input() {
        let root = temp_root("store");
        let input = base_input();
        let view = refresh_for_game(&root, &input, "2026-11-19T10:00:00Z").unwrap();
        assert!(view.stored);
        assert!(!view.stale);
        assert_eq!(view.plan.game_id, 7);

        let record = load_record(&root, 7).unwrap();
        assert_eq!(record.fingerprint, input.fingerprint());
        assert_eq!(record.plan.sentence, view.plan.sentence);
        assert_eq!(record.saved_at, "2026-11-19T10:00:00Z");

        let mut changed = input.clone();
        changed.gpu.vendor = GpuVendor::Nvidia;
        changed.gpu.driver = DriverFamily::NvidiaProprietary;
        changed.gpu.driver_name = "nvidia".to_string();
        let stale = view_for_game(&root, &changed, "2026-11-19T10:05:00Z");
        assert!(stale.stale);
        assert!(stale.stored);

        remove_record(&root, 7);
        assert!(load_record(&root, 7).is_none());
    }

    #[test]
    fn a_stored_plan_is_reused_verbatim_while_the_inputs_match() {
        let root = temp_root("reuse");
        let input = base_input();
        let stored = refresh_for_game(&root, &input, "2026-11-19T10:00:00Z").unwrap();
        let again = view_for_game(&root, &input, "2026-11-19T11:00:00Z");
        assert_eq!(again.plan.generated_at, stored.plan.generated_at);
        assert_eq!(again.plan.sentence, stored.plan.sentence);
    }

    #[test]
    fn the_export_is_portable_json_with_the_plan() {
        let plan = evaluate(&base_input(), &BuiltinRules);
        let exported = export_plan(&plan).unwrap();
        assert!(exported.contains("\"kind\": \"ludex-launch-plan\""));
        assert!(exported.contains(&plan.sentence));
        assert!(!exported.contains("/home/"));
    }

    #[test]
    fn prerequisites_run_once_per_recipe_and_restart_when_the_recipe_changes() {
        let mut input = base_input();
        input.declared_prerequisites = vec!["vcrun".to_string(), "directx".to_string()];
        let plan = evaluate(&input, &BuiltinRules);

        let pending = pending_prerequisites(&plan, None);
        assert_eq!(pending.len(), 2);

        let marker = PrerequisiteMarker {
            fingerprint: recipe_fingerprint(&plan.prerequisites),
            applied: vec!["vcrun".to_string()],
            failed: Vec::new(),
            updated_at: "2026-11-19T10:00:00Z".to_string(),
        };
        let pending = pending_prerequisites(&plan, Some(&marker));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "directx");

        let marker = PrerequisiteMarker {
            applied: vec!["vcrun".to_string(), "directx".to_string()],
            ..marker
        };
        assert!(pending_prerequisites(&plan, Some(&marker)).is_empty());

        let marker = PrerequisiteMarker {
            fingerprint: "other".to_string(),
            applied: vec!["vcrun".to_string()],
            failed: Vec::new(),
            updated_at: String::new(),
        };
        assert_eq!(pending_prerequisites(&plan, Some(&marker)).len(), 2);
    }

    #[test]
    fn the_marker_round_trips_inside_the_prefix() {
        let root = temp_root("marker");
        let marker = PrerequisiteMarker {
            fingerprint: "abc".to_string(),
            applied: vec!["vcrun".to_string()],
            failed: Vec::new(),
            updated_at: "2026-11-19T10:00:00Z".to_string(),
        };
        assert!(read_marker(&root).is_none());
        write_marker(&root, &marker).unwrap();
        assert_eq!(read_marker(&root), Some(marker));
        // The marker never lands inside drive_c.
        assert!(root.join(".ludex/prerequisites.json").is_file());
    }

    #[test]
    fn a_failure_is_described_from_the_recovery_decision() {
        // #298 owns the classification; the plan only explains the decision.
        let fatal = PlanFailure::from_recovery(Some(53), 3, 30, true);
        assert_eq!(fatal.kind, FailureKind::FatalExit);
        assert_eq!(fatal.exit_code, 53);
        assert_eq!(fatal.budget_secs, 30);
        assert!(fatal.hint.contains("Steam emulator"));

        let no_frame = PlanFailure::from_recovery(Some(1), 12, 30, false);
        assert_eq!(no_frame.kind, FailureKind::NoFirstFrame);
        assert_eq!(no_frame.exit_code, 1);

        // A launch killed by a signal reports no code; the description stays
        // honest and keeps the fatal flag the classifier produced.
        let unknown = PlanFailure::from_recovery(None, 2, 30, false);
        assert_eq!(unknown.exit_code, 0);
        assert!(unknown.hint.contains("exited before the game started"));
    }

    #[test]
    fn an_engine_game_records_its_expected_startup_budget() {
        let mut input = base_input();
        input.engine = Some(Engine::Unity);
        let plan = evaluate(&input, &BuiltinRules);
        assert_eq!(plan.first_frame_budget_secs, 15);
        // The recovery classifier keeps its own documented 30 s window; the
        // plan budget is the expectation shown to support, not a decision.
        assert!(plan.first_frame_budget_secs < 30);
    }

    #[test]
    fn the_plan_is_written_into_the_launch_log() {
        let plan = evaluate(&base_input(), &BuiltinRules);
        let lines = log_lines(&plan);
        assert!(lines[0].starts_with("plan: Launching with UMU"));
        assert!(lines.iter().any(|line| line.contains("plan rules:")));
        assert!(lines.iter().any(|line| line.contains("plan source: auto")));
    }

    #[test]
    fn the_plan_section_is_appended_to_a_prepared_launch_log() {
        let root = temp_root("append-log");
        let log = root.join("launch-7-session.log");
        write(
            &log,
            "=== Ludex compatibility launch ===\n=== tool output ===\n",
        );
        let plan = evaluate(&base_input(), &BuiltinRules);
        append_log(&log, &plan).unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with(
            "=== Ludex compatibility launch ===\n=== tool output ===\n--- launch plan ---\n"
        ));
        assert!(text.contains(&plan.sentence));
        assert!(text.contains("plan rules: windows-exe-umu"));
        assert!(text.contains("plan source: auto"));
    }

    #[test]
    fn a_manual_plan_says_so_in_the_log() {
        let mut input = base_input();
        input.manual_override = true;
        let plan = evaluate(&input, &BuiltinRules);
        let lines = log_lines(&plan);
        assert!(lines
            .iter()
            .any(|line| line.contains("manual override (plan not applied)")));
    }

    #[test]
    fn gamemoderun_wraps_the_entry_point_only_when_asked() {
        let wrappers = Wrappers {
            gamemode: true,
            ..Wrappers::default()
        };
        let (program, args) = wrap_entry(
            PathBuf::from("/usr/bin/umu-run"),
            vec!["/games/Hades.exe".to_string()],
            &wrappers,
            Some(PathBuf::from("/usr/bin/gamemoderun")),
        );
        assert_eq!(program, PathBuf::from("/usr/bin/gamemoderun"));
        assert_eq!(args, vec!["/usr/bin/umu-run", "/games/Hades.exe"]);

        let wrappers = Wrappers::default();
        let (program, args) = wrap_entry(
            PathBuf::from("/usr/bin/umu-run"),
            vec!["/games/Hades.exe".to_string()],
            &wrappers,
            Some(PathBuf::from("/usr/bin/gamemoderun")),
        );
        assert_eq!(program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(args, vec!["/games/Hades.exe"]);
    }

    #[test]
    fn wrapper_detection_reads_a_path_fixture() {
        let root = temp_root("wrappers");
        write(&root.join("gamemoderun"), "");
        let (gamemode, mangohud) = probe_wrappers(std::slice::from_ref(&root));
        assert!(gamemode);
        assert!(!mangohud);
    }

    #[test]
    fn the_input_fingerprint_changes_with_every_decision_field() {
        let input = base_input();
        let base = input.fingerprint();
        assert_eq!(base.len(), 24);

        let mut changed = input.clone();
        changed.known_game_id = None;
        assert_ne!(base, changed.fingerprint());

        let mut changed = input.clone();
        changed.manual_override = true;
        assert_ne!(base, changed.fingerprint());

        assert_eq!(base, input.fingerprint());
    }
}
