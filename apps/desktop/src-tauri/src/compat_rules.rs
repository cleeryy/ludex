//! Compatibility rules database (#301).
//!
//! A rule is data: it matches facts Ludex already knows about a game and this
//! machine, then fills in launch defaults with a human reason. [`SHIPPED_RULES_TOML`]
//! is embedded at build time; the same TOML schema is also accepted from a
//! signed download (`compat_rules_update`) and from a local override file
//! (`compat-rules.local.toml`), so a rule reaches users without an app release.
//!
//! The module is deliberately narrow, because two other issues consume it:
//!
//! ```text
//! #297 launch planner               #299 settings surface
//!        │                                  │
//!        └── load() → RuleSet ──┬────────────┘
//!                               ├─ rule_matches(rule, input) -> bool
//!                               └─ evaluate(set, input, base) -> the plan
//! ```
//!
//! [`load`] never talks to the network and never fails: a malformed or missing
//! layer is skipped, logged once and the next layer (or the shipped defaults)
//! still loads. [`update`] is the only network path; it runs at startup in a
//! detached task, verifies a minisign signature with the updater key, and only
//! then replaces the cache. A launch never waits for it.
//!
//! Precedence, highest first:
//!   1. the user's own launch options (a rule reports `user-choice` instead of
//!      overwriting a value the user set);
//!   2. local rules from `compat-rules.local.toml` ("learned locally"), which
//!      may match a local `game_id`;
//!   3. the signed cache (`compat-rules.toml` + `.minisig`), when it validates
//!      and is not older than the shipped file;
//!   4. the rules embedded in this build.
//!
//! Plan overrides are limited to fields that exist on [`LaunchOptions`] or its
//! [`crate::linux_gaming::DisplayProfile`]. Machine-specific paths (`prefix`,
//! `tool_id`, `working_dir`) are refused on purpose: a shared rule that carries
//! them would be a dead key on every other machine. `args` is returned beside
//! the options because the launch call takes extra arguments separately; #297
//! appends them after the user's own startup command.
//!
//! Validation is the contract, and it is enforced by tests against the shipped
//! file: schema version, unique ids, a non-empty match, a reason, a source, a
//! confidence, known-issue codes, prerequisites from the runtime planner, and
//! every plan key mapped to a real field. `docs/COMPAT-RULES.md` documents the
//! schema for contributors.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::db;
use crate::linux_gaming::{
    self, DisplayMode, DisplayProfile, LaunchMode, LaunchOptions, DISPLAY_FILTERS,
    DISPLAY_HEIGHT_RANGE, DISPLAY_MAX_REFRESH, DISPLAY_WIDTH_RANGE,
};
use crate::runtime::plan::PREREQUISITE_IDS;
use crate::runtime::Platform;
use crate::state::AppState;

/// Schema this build understands. A file with another number is refused.
pub const SCHEMA_VERSION: u32 = 1;

/// Rules embedded in this build. Validated by the tests below.
pub const SHIPPED_RULES_TOML: &str = include_str!("../../../../data/compat-rules.toml");

/// Signed rules, fetched from `main` so the database updates between releases.
pub const RULES_URL: &str =
    "https://raw.githubusercontent.com/cleeryy/ludex/main/data/compat-rules.toml";
/// Detached minisign signature for [`RULES_URL`].
pub const RULES_SIGNATURE_URL: &str =
    "https://raw.githubusercontent.com/cleeryy/ludex/main/data/compat-rules.toml.minisig";

/// Cache filename inside the app-data folder.
pub const CACHE_FILE: &str = "compat-rules.toml";
/// Local override filename inside the app-data folder ("learned locally").
pub const LOCAL_FILE: &str = "compat-rules.local.toml";
const CACHE_SIGNATURE_FILE: &str = "compat-rules.toml.minisig";
const STATE_FILE: &str = "compat-rules.state.json";

/// A downloaded rules file above this size is refused before parsing.
const MAX_RULES_BYTES: usize = 512 * 1024;
/// Automatic refresh interval; an explicit update ignores it.
const AUTO_REFRESH_HOURS: i64 = 24;
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RULES: usize = 500;
const MAX_REASON_CHARS: usize = 800;
const MAX_TEXT_CHARS: usize = 1_200;
const MAX_SOURCE_CHARS: usize = 240;
const MAX_ENV_VARS: usize = 16;
const MAX_ARGS: usize = 16;
const MAX_ARG_CHARS: usize = 200;
const MAX_ISSUES_PER_RULE: usize = 8;
/// Executables above this size are not hashed for `exe_sha256` matching.
const MAX_HASH_BYTES: u64 = 4 * 1024 * 1024 * 1024;

const ARCHES: [&str; 3] = ["x86_64", "aarch64", "x86"];

/* ------------------------------------------------------------------ data */

/// How sure a rule is of itself, shown in the "why" panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Follows from the launch model or the executable header.
    Certain,
    /// Depends on a configuration outside Ludex.
    Likely,
    /// A heuristic that must be confirmable by the user.
    Guess,
}

/// GPU vendor a rule can target. `unknown` is a real condition: no probe
/// answered, and a rule may want to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

impl GpuVendor {
    /// Vendors only match a rule when the caller supplied a fact.
    #[must_use]
    pub fn from_hint(hint: &str) -> Self {
        let lower = hint.trim().to_ascii_lowercase();
        if lower.contains("nvidia") {
            GpuVendor::Nvidia
        } else if lower.contains("amd") || lower.contains("ati") || lower.contains("radeon") {
            GpuVendor::Amd
        } else if lower.contains("intel") {
            GpuVendor::Intel
        } else {
            GpuVendor::Unknown
        }
    }
}

/// Host platform in the rules vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RulePlatform {
    Linux,
    Windows,
    Macos,
}

impl From<Platform> for RulePlatform {
    fn from(platform: Platform) -> Self {
        match platform {
            Platform::Linux => RulePlatform::Linux,
            Platform::Windows => RulePlatform::Windows,
            Platform::Macos => RulePlatform::Macos,
        }
    }
}

impl RulePlatform {}

/// What the chosen executable is, from the PE header when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutableKind {
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "windows")]
    Windows,
    #[serde(rename = "windows-arm64")]
    WindowsArm64,
    #[serde(rename = "unknown")]
    Unknown,
}

impl ExecutableKind {}

/// Severity of a known issue. Only `blocking` means "this cannot start".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueSeverity {
    Info,
    Warning,
    Blocking,
}

impl IssueSeverity {}

/// One documented issue attached to a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownIssue {
    /// Stable slug the UI can key on; unique within a rule.
    pub code: String,
    pub severity: IssueSeverity,
    /// One sentence, data, shown with the rule.
    pub detail: String,
}

/// Conditions a rule needs. Every present condition must hold (logical AND).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleMatch {
    /// Local game id. Local rules only: server ids are not portable.
    #[serde(default)]
    pub game_id: Option<i64>,
    /// Lowercase SHA-256 of the executable.
    #[serde(default)]
    pub exe_sha256: Option<String>,
    /// Engine tag (`unity`, `unreal`, `godot`, …), compared case-insensitively.
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub gpu_vendor: Option<GpuVendor>,
    #[serde(default)]
    pub platform: Option<RulePlatform>,
    /// `x86_64`, `aarch64` or `x86`.
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub executable: Option<ExecutableKind>,
}

impl RuleMatch {
    fn is_empty(&self) -> bool {
        self.game_id.is_none()
            && self.exe_sha256.is_none()
            && self.engine.is_none()
            && self.gpu_vendor.is_none()
            && self.platform.is_none()
            && self.arch.is_none()
            && self.executable.is_none()
    }
}

/// Plan fields a rule may override.
///
/// Every key maps to a stored field. `prefix_path`, `tool_id` and `working_dir`
/// are absent on purpose: they name a machine, so a shared rule carrying one
/// would never apply anywhere else.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanOverrides {
    #[serde(default)]
    pub mode: Option<LaunchMode>,
    /// Appended after the user's own arguments by the launch planner (#297).
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Stored as the per-game verb list; running them stays an explicit step.
    #[serde(default)]
    pub winetricks: Vec<String>,
    #[serde(default)]
    pub steam_app_id: Option<u32>,
    /// A complete Gamescope profile; validation refuses a partial one.
    #[serde(default)]
    pub display: Option<DisplayPatch>,
}

impl PlanOverrides {
    fn is_empty(&self) -> bool {
        self.mode.is_none()
            && self.args.is_empty()
            && self.env.is_empty()
            && self.winetricks.is_empty()
            && self.steam_app_id.is_none()
            && self.display.is_none()
    }
}

/// A Gamescope profile written as a delta, validated as a whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPatch {
    #[serde(default)]
    pub mode: Option<DisplayMode>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub refresh: Option<u32>,
    #[serde(default)]
    pub filter: Option<String>,
    #[serde(default)]
    pub adaptive_sync: Option<bool>,
    #[serde(default)]
    pub hdr: Option<bool>,
}

/// One row of the database.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatRule {
    /// Stable slug; a local rule with the same id replaces the shipped one.
    pub id: String,
    /// The human sentence shown in the "why" panel.
    pub reason: String,
    pub confidence: Confidence,
    /// Where the claim comes from: a code path, a measurement, a document.
    #[serde(default)]
    pub source: String,
    #[serde(rename = "match")]
    pub matches: RuleMatch,
    #[serde(default)]
    pub plan: PlanOverrides,
    /// Redistributable ids from [`PREREQUISITE_IDS`].
    #[serde(default)]
    pub prerequisites: Vec<String>,
    #[serde(default)]
    pub known_issues: Vec<KnownIssue>,
}

/// A whole rules file: shipped, cached or local.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesDocument {
    pub schema: u32,
    /// Display label: `YYYY.MM` or `YYYY.MM.N` ("rule 2026.11").
    pub version: String,
    #[serde(default)]
    pub updated: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub rules: Vec<CompatRule>,
}

/// The local override file, which may also be a bare list of rules.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalRulesDocument {
    #[serde(default)]
    schema: Option<u32>,
    #[serde(default, rename = "version")]
    _version: Option<String>,
    #[serde(default, rename = "updated")]
    _updated: Option<String>,
    #[serde(default, rename = "description")]
    _description: String,
    #[serde(default)]
    rules: Vec<CompatRule>,
}

/// Where a rule came from. The UI turns this into a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleOrigin {
    /// Embedded in this build.
    Shipped,
    /// Downloaded, signature-checked copy.
    Cached,
    /// `compat-rules.local.toml`, written on this machine.
    Local,
}

impl RuleOrigin {}

/// A rule plus the layer it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleEntry {
    pub origin: RuleOrigin,
    pub rule: CompatRule,
}

/// Everything [`load`] could assemble, never an error.
///
/// `version` is the base database label (`2026.11`); local rules carry no
/// version, they are labelled "learned locally".
#[derive(Debug, Clone)]
pub struct RuleSet {
    pub document: RulesDocument,
    pub origin: RuleOrigin,
    pub entries: Vec<RuleEntry>,
    /// Degradation notes for logs and diagnostics (English, not UI copy).
    pub notes: Vec<String>,
    /// True when any layer was skipped or the cache was ignored.
    pub degraded: bool,
}

impl RuleSet {
    /// The label of the active database, `2026.11` for the shipped file.
    pub fn version(&self) -> &str {
        &self.document.version
    }

    pub fn local_rule_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.origin == RuleOrigin::Local)
            .count()
    }

    /// Rules in evaluation order: local first (they win conflicts), then the
    /// base database minus any id a local rule replaced.
    pub fn entries(&self) -> &[RuleEntry] {
        &self.entries
    }
}

/* --------------------------------------------------------------- parsing */

/// Parse and validate a rules file. Returns every problem, not the first.
pub fn parse_rules(text: &str, origin: RuleOrigin) -> Result<RulesDocument, Vec<String>> {
    let document: RulesDocument = match toml::from_str(text) {
        Ok(document) => document,
        Err(error) => return Err(vec![format!("TOML: {error}")]),
    };
    let errors = validate(&document, origin);
    if errors.is_empty() {
        Ok(document)
    } else {
        Err(errors)
    }
}

fn parse_local_rules(text: &str) -> Result<Vec<CompatRule>, Vec<String>> {
    let document: LocalRulesDocument = match toml::from_str(text) {
        Ok(document) => document,
        Err(error) => return Err(vec![format!("TOML: {error}")]),
    };
    if let Some(schema) = document.schema {
        if schema != SCHEMA_VERSION {
            return Err(vec![format!(
                "schema {schema} is not supported; this build understands schema {SCHEMA_VERSION}"
            )]);
        }
    }
    if document.rules.len() > MAX_RULES {
        return Err(vec![format!(
            "{} rules; at most {MAX_RULES} are accepted",
            document.rules.len()
        )]);
    }
    let mut errors = Vec::new();
    let mut ids = BTreeSet::new();
    for rule in &document.rules {
        validate_rule(rule, RuleOrigin::Local, &mut ids, &mut errors);
    }
    if errors.is_empty() {
        Ok(document.rules)
    } else {
        Err(errors)
    }
}

fn validate(document: &RulesDocument, origin: RuleOrigin) -> Vec<String> {
    let mut errors = Vec::new();
    if document.schema != SCHEMA_VERSION {
        return vec![format!(
            "schema {} is not supported; this build understands schema {SCHEMA_VERSION}",
            document.schema
        )];
    }
    if parse_version_key(&document.version).is_none() {
        errors.push(format!(
            "version {:?} is not YYYY.MM or YYYY.MM.N",
            document.version
        ));
    }
    if !document.updated.is_empty()
        && chrono::NaiveDate::parse_from_str(&document.updated, "%Y-%m-%d").is_err()
    {
        errors.push(format!(
            "updated {:?} is not a YYYY-MM-DD date",
            document.updated
        ));
    }
    if document.rules.is_empty() {
        errors.push("the file declares no rules".to_string());
    }
    if document.rules.len() > MAX_RULES {
        errors.push(format!(
            "{} rules; at most {MAX_RULES} are accepted",
            document.rules.len()
        ));
    }
    let mut ids = BTreeSet::new();
    for rule in &document.rules {
        validate_rule(rule, origin, &mut ids, &mut errors);
    }
    errors
}

fn validate_rule(
    rule: &CompatRule,
    origin: RuleOrigin,
    ids: &mut BTreeSet<String>,
    errors: &mut Vec<String>,
) {
    let prefix = if rule.id.trim().is_empty() {
        "rule (no id)".to_string()
    } else {
        format!("rule {}", rule.id)
    };
    if !is_slug(&rule.id) {
        errors.push(format!("{prefix}: id must match [a-z0-9][a-z0-9-]{{2,63}}"));
    }
    if !ids.insert(rule.id.clone()) {
        errors.push(format!("{prefix}: duplicate id"));
    }
    if rule.reason.trim().is_empty() {
        errors.push(format!("{prefix}: reason is empty"));
    } else if rule.reason.chars().count() > MAX_REASON_CHARS {
        errors.push(format!(
            "{prefix}: reason is longer than {MAX_REASON_CHARS} characters"
        ));
    }
    if rule.source.trim().is_empty() && origin != RuleOrigin::Local {
        errors.push(format!("{prefix}: source is empty"));
    }
    if rule.source.chars().count() > MAX_SOURCE_CHARS {
        errors.push(format!(
            "{prefix}: source is longer than {MAX_SOURCE_CHARS} characters"
        ));
    }
    if rule.matches.is_empty() {
        errors.push(format!(
            "{prefix}: match is empty; a rule must name at least one condition"
        ));
    }
    if origin != RuleOrigin::Local && rule.matches.game_id.is_some() {
        errors.push(format!(
            "{prefix}: a shipped rule cannot match a local game_id; use exe_sha256 or a fact"
        ));
    }
    if let Some(hash) = &rule.matches.exe_sha256 {
        if !is_sha256(hash) {
            errors.push(format!(
                "{prefix}: exe_sha256 must be 64 lowercase hex characters"
            ));
        }
    }
    if let Some(engine) = &rule.matches.engine {
        if !is_engine_tag(engine) {
            errors.push(format!(
                "{prefix}: engine {engine:?} must match [a-z0-9][a-z0-9._-]{{0,31}}"
            ));
        }
    }
    if let Some(arch) = &rule.matches.arch {
        let normalized = crate::runtime::detect::normalize_arch(arch);
        if !ARCHES.contains(&normalized.as_str()) {
            errors.push(format!(
                "{prefix}: arch {arch:?} must be one of {}",
                ARCHES.join(", ")
            ));
        }
    }
    validate_plan(rule, &prefix, errors);
    for prerequisite in &rule.prerequisites {
        if !PREREQUISITE_IDS.contains(&prerequisite.as_str()) {
            errors.push(format!(
                "{prefix}: prerequisite {prerequisite:?} is not one of {}",
                PREREQUISITE_IDS.join(", ")
            ));
        }
    }
    if has_duplicates(&rule.prerequisites) {
        errors.push(format!("{prefix}: prerequisites contain a duplicate"));
    }
    if rule.known_issues.len() > MAX_ISSUES_PER_RULE {
        errors.push(format!(
            "{prefix}: {} known issues; at most {MAX_ISSUES_PER_RULE} are accepted",
            rule.known_issues.len()
        ));
    }
    let mut codes = BTreeSet::new();
    for issue in &rule.known_issues {
        if !is_slug(&issue.code) {
            errors.push(format!(
                "{prefix}: known issue code {:?} must match [a-z0-9][a-z0-9-]{{2,63}}",
                issue.code
            ));
        }
        if !codes.insert(issue.code.clone()) {
            errors.push(format!("{prefix}: duplicate known issue {:?}", issue.code));
        }
        if issue.detail.trim().is_empty() {
            errors.push(format!(
                "{prefix}: known issue {:?} has no detail",
                issue.code
            ));
        } else if issue.detail.chars().count() > MAX_TEXT_CHARS {
            errors.push(format!(
                "{prefix}: known issue {:?} is longer than {MAX_TEXT_CHARS} characters",
                issue.code
            ));
        }
    }
    if rule.plan.is_empty() && rule.prerequisites.is_empty() && rule.known_issues.is_empty() {
        errors.push(format!(
            "{prefix}: a rule must change the plan, declare a prerequisite or report a known issue"
        ));
    }
}

fn validate_plan(rule: &CompatRule, prefix: &str, errors: &mut Vec<String>) {
    if rule.plan.args.len() > MAX_ARGS {
        errors.push(format!(
            "{prefix}: {} args; at most {MAX_ARGS} are accepted",
            rule.plan.args.len()
        ));
    }
    for argument in &rule.plan.args {
        if argument.trim().is_empty() || argument.len() > MAX_ARG_CHARS {
            errors.push(format!(
                "{prefix}: arg {argument:?} is empty or longer than {MAX_ARG_CHARS} bytes"
            ));
        } else if argument.contains('\0') {
            errors.push(format!("{prefix}: arg {argument:?} contains a NUL byte"));
        }
    }
    if rule.plan.env.len() > MAX_ENV_VARS {
        errors.push(format!(
            "{prefix}: {} environment variables; at most {MAX_ENV_VARS} are accepted",
            rule.plan.env.len()
        ));
    } else if !rule.plan.env.is_empty() {
        match linux_gaming::validate_env(&rule.plan.env) {
            Ok(cleaned) => {
                if cleaned.len() != rule.plan.env.len() {
                    errors.push(format!(
                        "{prefix}: env contains an empty key or a silent duplicate"
                    ));
                }
                for (key, value) in &rule.plan.env {
                    if cleaned.get(key) != Some(value) {
                        errors.push(format!(
                            "{prefix}: env {key:?} is not in normal form (trim the key, keep the value)"
                        ));
                    }
                }
            }
            Err(error) => errors.push(format!("{prefix}: {error}")),
        }
    }
    if !rule.plan.winetricks.is_empty() {
        match linux_gaming::validate_verbs(&rule.plan.winetricks) {
            Ok(cleaned) => {
                if cleaned != rule.plan.winetricks {
                    errors.push(format!(
                        "{prefix}: winetricks verbs must be lowercase, unique and already normalised"
                    ));
                }
            }
            Err(error) => errors.push(format!("{prefix}: {error}")),
        }
    }
    if let Some(app_id) = rule.plan.steam_app_id {
        if app_id == 0 {
            errors.push(format!("{prefix}: steam_app_id must be a positive app id"));
        }
    }
    if let Some(display) = &rule.plan.display {
        let mode = display.mode.unwrap_or(DisplayMode::Desktop);
        let has_other = display.width.is_some()
            || display.height.is_some()
            || display.refresh.is_some()
            || display.filter.is_some()
            || display.adaptive_sync.is_some()
            || display.hdr.is_some();
        match mode {
            DisplayMode::Desktop => {
                if has_other {
                    errors.push(format!(
                        "{prefix}: a display override needs mode = \"gamescope\"; a desktop patch is inert"
                    ));
                }
            }
            DisplayMode::Gamescope => {
                let width = display.width.unwrap_or_default();
                let height = display.height.unwrap_or_default();
                if !DISPLAY_WIDTH_RANGE.contains(&width) {
                    errors.push(format!(
                        "{prefix}: display.width must be between {} and {}",
                        DISPLAY_WIDTH_RANGE.start(),
                        DISPLAY_WIDTH_RANGE.end()
                    ));
                }
                if !DISPLAY_HEIGHT_RANGE.contains(&height) {
                    errors.push(format!(
                        "{prefix}: display.height must be between {} and {}",
                        DISPLAY_HEIGHT_RANGE.start(),
                        DISPLAY_HEIGHT_RANGE.end()
                    ));
                }
                if let Some(refresh) = display.refresh {
                    if refresh > DISPLAY_MAX_REFRESH {
                        errors.push(format!(
                            "{prefix}: display.refresh must be 0 or at most {DISPLAY_MAX_REFRESH}"
                        ));
                    }
                }
                if let Some(filter) = &display.filter {
                    if !filter.is_empty() && !DISPLAY_FILTERS.contains(&filter.as_str()) {
                        errors.push(format!(
                            "{prefix}: display.filter {filter:?} is not one of {}",
                            DISPLAY_FILTERS.join(", ")
                        ));
                    }
                }
            }
        }
    }
}

fn is_slug(value: &str) -> bool {
    let mut chars = value.chars();
    let valid_start = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
    valid_start
        && value.len() >= 3
        && value.len() <= 64
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase())
}

fn is_engine_tag(value: &str) -> bool {
    let mut chars = value.chars();
    let valid_start = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase() || character.is_ascii_digit());
    valid_start
        && value.len() <= 32
        && chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
}

fn has_duplicates(values: &[String]) -> bool {
    let mut seen = BTreeSet::new();
    values.iter().any(|value| !seen.insert(value))
}

/// Parse `YYYY.MM` (optionally `.N`) into a comparable number.
#[must_use]
pub fn parse_version_key(version: &str) -> Option<u64> {
    let mut parts = version.trim().split('.');
    let year: u64 = parts.next()?.parse().ok()?;
    let month: u64 = parts.next()?.parse().ok()?;
    if !(2000..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let patch: u64 = match parts.next() {
        Some(value) => value.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() || patch > 99 {
        return None;
    }
    Some((year * 10_000 + month * 100 + patch) * 100)
}

/* --------------------------------------------------------------- loading */

/// Read the rules once for the whole app data folder.
///
/// `public_key` is the minisign public key text (`untrusted comment:` line plus
/// the key). Without it, a cache is ignored: unverified data never drives a
/// launch, and the shipped file is used instead. This function never performs
/// I/O outside `data_dir` and never returns an error.
#[must_use]
pub fn load_from(data_dir: &Path, public_key: Option<&str>) -> RuleSet {
    let mut notes = Vec::new();
    let mut degraded = false;

    let shipped = match parse_rules(SHIPPED_RULES_TOML, RuleOrigin::Shipped) {
        Ok(document) => document,
        Err(errors) => {
            let message = format!("shipped rules are invalid: {}", errors.join("; "));
            log_once("shipped-invalid", &message);
            degraded = true;
            notes.push(message);
            RulesDocument {
                schema: SCHEMA_VERSION,
                version: "0.0".to_string(),
                updated: String::new(),
                description: String::new(),
                rules: Vec::new(),
            }
        }
    };

    let mut base = shipped;
    let mut origin = RuleOrigin::Shipped;

    let cache = if data_dir.as_os_str().is_empty() {
        Ok(None)
    } else {
        load_cache(data_dir, public_key)
    };
    match cache {
        Ok(Some((document, cached_version))) => {
            let shipped_key = parse_version_key(&base.version).unwrap_or_default();
            let cached_key = parse_version_key(&cached_version).unwrap_or_default();
            if cached_key >= shipped_key {
                base = document;
                origin = RuleOrigin::Cached;
            } else {
                notes.push(format!(
                    "cached rules {cached_version} are older than the shipped {}; the shipped file wins",
                    base.version
                ));
                degraded = true;
            }
        }
        Ok(None) => {}
        Err(note) => {
            notes.push(note);
            degraded = true;
        }
    }

    let local_path = data_dir.join(LOCAL_FILE);
    let local = if data_dir.as_os_str().is_empty() {
        Vec::new()
    } else {
        match std::fs::read_to_string(&local_path) {
            Ok(text) => match parse_local_rules(&text) {
                Ok(rules) => rules,
                Err(errors) => {
                    let message = format!(
                        "{} is invalid and was ignored: {}",
                        local_path.display(),
                        errors.join("; ")
                    );
                    log_once("local-invalid", &message);
                    notes.push(message);
                    degraded = true;
                    Vec::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                let message = format!("{} could not be read: {error}", local_path.display());
                log_once("local-unreadable", &message);
                notes.push(message);
                degraded = true;
                Vec::new()
            }
        }
    };

    for note in &notes {
        tracing::debug!(note = %note, "compatibility rules: degraded layer");
    }

    let entries = merge_rules(&base, local);
    RuleSet {
        document: base,
        origin,
        entries,
        notes,
        degraded,
    }
}

fn load_cache(
    data_dir: &Path,
    public_key: Option<&str>,
) -> Result<Option<(RulesDocument, String)>, String> {
    let path = data_dir.join(CACHE_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    let Some(public_key) = public_key else {
        return Err("the downloaded rules are ignored: this build has no signing key".to_string());
    };
    let signature_path = data_dir.join(CACHE_SIGNATURE_FILE);
    let signature = std::fs::read_to_string(&signature_path).map_err(|error| {
        format!(
            "{} is missing or unreadable: {error}",
            signature_path.display()
        )
    })?;
    verify_signature(text.as_bytes(), &signature, public_key).map_err(|error| {
        format!(
            "the downloaded rules failed signature verification ({error}); the shipped file is used"
        )
    })?;
    let document = parse_rules(&text, RuleOrigin::Cached)
        .map_err(|errors| format!("the downloaded rules are invalid: {}", errors.join("; ")))?;
    let version = document.version.clone();
    Ok(Some((document, version)))
}

/// Local rules replace a shipped rule with the same id and are evaluated
/// first, so a local rule also wins a conflict between two different ids.
fn merge_rules(base: &RulesDocument, local: Vec<CompatRule>) -> Vec<RuleEntry> {
    let replaced: BTreeSet<String> = local.iter().map(|rule| rule.id.clone()).collect();
    let mut entries: Vec<RuleEntry> = local
        .into_iter()
        .map(|rule| RuleEntry {
            origin: RuleOrigin::Local,
            rule,
        })
        .collect();
    entries.extend(
        base.rules
            .iter()
            .filter(|rule| !replaced.contains(&rule.id))
            .cloned()
            .map(|rule| RuleEntry {
                origin: RuleOrigin::Shipped,
                rule,
            }),
    );
    entries
}

/* ------------------------------------------------------------ signature */

/// Verify a minisign signature over the exact bytes of a rules file.
///
/// `public_key_text` is the two-line `minisign.pub` form. The Tauri updater
/// stores the same text base64-encoded in `tauri.conf.json`; use
/// [`tauri_public_key`] to decode that form.
pub fn verify_signature(
    content: &[u8],
    signature: &str,
    public_key_text: &str,
) -> Result<(), String> {
    let key = minisign_verify::PublicKey::decode(public_key_text)
        .map_err(|error| format!("invalid public key: {error}"))?;
    let signature = decode_signature(signature)?;
    key.verify(content, &signature, false)
        .map_err(|error| format!("signature rejected: {error}"))
}

/// Decode either form Tauri accepts: the four-line minisign text, or the
/// base64 of that text (the `signature` field of `latest.json` and the `.sig`
/// file `tauri signer sign` writes).
fn decode_signature(signature: &str) -> Result<minisign_verify::Signature, String> {
    let text = signature.trim();
    if text.starts_with("untrusted comment") || text.lines().count() >= 4 {
        return minisign_verify::Signature::decode(text)
            .map_err(|error| format!("invalid signature file: {error}"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|error| format!("invalid signature encoding: {error}"))?;
    let decoded = String::from_utf8(bytes)
        .map_err(|_| "the signature is not UTF-8 after decoding".to_string())?;
    minisign_verify::Signature::decode(&decoded)
        .map_err(|error| format!("invalid signature file: {error}"))
}

/// Decode the base64 key Tauri keeps under `plugins.updater.pubkey`.
#[must_use]
pub fn tauri_public_key(encoded: &str) -> Option<String> {
    let encoded = encoded.trim();
    if encoded.is_empty() {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

fn configured_public_key(app: &AppHandle) -> Option<String> {
    let encoded = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(|value| value.as_str())?;
    tauri_public_key(encoded)
}

/* -------------------------------------------------------------- matching */

/// Facts the matcher reads. Built by the caller (#297 at launch, the "why"
/// command for the panel) so matching stays pure and testable.
#[derive(Debug, Clone)]
pub struct MatchInput {
    pub game_id: i64,
    pub exe_sha256: Option<String>,
    pub engine: Option<String>,
    pub platform: RulePlatform,
    /// Normalised with [`crate::runtime::detect::normalize_arch`].
    pub arch: String,
    pub gpu_vendor: GpuVendor,
    pub executable: ExecutableKind,
}

impl MatchInput {
    #[must_use]
    pub fn for_host(game_id: i64) -> Self {
        Self {
            game_id,
            exe_sha256: None,
            engine: None,
            platform: RulePlatform::from(Platform::current()),
            arch: crate::runtime::detect::normalize_arch(std::env::consts::ARCH),
            gpu_vendor: GpuVendor::Unknown,
            executable: ExecutableKind::Unknown,
        }
    }
}

/// True when every condition the rule names holds.
#[must_use]
pub fn rule_matches(rule: &CompatRule, input: &MatchInput) -> bool {
    let conditions = &rule.matches;
    if let Some(game_id) = conditions.game_id {
        if game_id != input.game_id {
            return false;
        }
    }
    if let Some(hash) = &conditions.exe_sha256 {
        let Some(actual) = input.exe_sha256.as_deref() else {
            return false;
        };
        if !actual.eq_ignore_ascii_case(hash) {
            return false;
        }
    }
    if let Some(engine) = &conditions.engine {
        let Some(actual) = input.engine.as_deref() else {
            return false;
        };
        if !actual.trim().eq_ignore_ascii_case(engine) {
            return false;
        }
    }
    if let Some(vendor) = conditions.gpu_vendor {
        if vendor != input.gpu_vendor {
            return false;
        }
    }
    if let Some(platform) = conditions.platform {
        if platform != input.platform {
            return false;
        }
    }
    if let Some(arch) = &conditions.arch {
        if crate::runtime::detect::normalize_arch(arch) != input.arch {
            return false;
        }
    }
    if let Some(kind) = conditions.executable {
        if kind != input.executable {
            return false;
        }
    }
    true
}

/// Which executable a path is, for matching.
///
/// A readable PE header wins. A file without one is `native` unless it still
/// ends in `.exe`, which is then `unknown` (a launcher stub, a truncated file).
#[must_use]
pub fn executable_kind(path: &Path) -> ExecutableKind {
    match crate::runtime::plan::pe_machine(path).as_deref() {
        Some("aarch64") => ExecutableKind::WindowsArm64,
        Some(_) => ExecutableKind::Windows,
        None => {
            let is_exe = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"));
            if is_exe {
                ExecutableKind::Unknown
            } else {
                ExecutableKind::Native
            }
        }
    }
}

/// Hash the executable when a rule can use the hash.
///
/// Returns `None` for an unreadable or oversized file: matching by properties
/// still works, and the caller reports the missing hash in the panel.
#[must_use]
pub fn executable_sha256(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_HASH_BYTES {
        return None;
    }
    crate::saves::sync::sha256_file(path).ok()
}

/* ------------------------------------------------------------- applying */

/// One field a rule wanted to set and whether it was applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleChange {
    /// Stable field name: `mode`, `env.PROTON_ENABLE_NVAPI`, `display.hdr`, …
    pub field: String,
    /// Human-readable value for the panel.
    pub value: String,
    pub applied: bool,
    /// `user-choice`, `rule-conflict` or `duplicate` when not applied.
    pub skipped: Option<String>,
}

impl RuleChange {
    fn applied(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
            applied: true,
            skipped: None,
        }
    }

    fn skipped(field: impl Into<String>, value: impl Into<String>, reason: &str) -> Self {
        Self {
            field: field.into(),
            value: value.into(),
            applied: false,
            skipped: Some(reason.to_string()),
        }
    }
}

/// A matched rule plus what it did, as the "why" panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluatedRule {
    pub id: String,
    pub origin: RuleOrigin,
    /// `2026.11` for a database rule, empty for a local one.
    pub rules_version: String,
    pub reason: String,
    pub confidence: Confidence,
    pub source: String,
    /// True when the rule changed the plan or declared a prerequisite.
    pub applied: bool,
    pub changes: Vec<RuleChange>,
    pub prerequisites: Vec<String>,
    pub known_issues: Vec<KnownIssue>,
}

/// A known issue flattened for the top-level list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedIssue {
    pub rule_id: String,
    pub code: String,
    pub severity: IssueSeverity,
    pub detail: String,
}

/// The result of applying matched rules to one base plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleEvaluation {
    /// Base options with every applied override. This is what a launch uses.
    pub options: LaunchOptions,
    /// Extra arguments, appended after the user's own (#297).
    pub extra_args: Vec<String>,
    /// Matched rules in evaluation order, each with its reason and changes.
    pub rules: Vec<EvaluatedRule>,
    /// Redistributable ids the rules asked for, deduplicated.
    pub prerequisites: Vec<String>,
    /// Every known issue from the matched rules.
    pub issues: Vec<ReportedIssue>,
}

/// Match and apply in one call. `base` is the stored plan, so a value the user
/// already set is never overwritten: the rule reports `user-choice` instead.
#[must_use]
pub fn evaluate(set: &RuleSet, input: &MatchInput, base: &LaunchOptions) -> RuleEvaluation {
    let mut options = base.clone();
    let mut extra_args: Vec<String> = Vec::new();
    let mut prerequisites: BTreeSet<String> = BTreeSet::new();
    let mut issues: Vec<ReportedIssue> = Vec::new();
    let mut rules: Vec<EvaluatedRule> = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();

    for entry in set.entries() {
        if !rule_matches(&entry.rule, input) {
            continue;
        }
        let (changes, added_prerequisites) =
            apply_rule(&entry.rule, &mut options, &mut extra_args, &mut claimed);
        prerequisites.extend(added_prerequisites.iter().cloned());
        issues.extend(entry.rule.known_issues.iter().map(|issue| ReportedIssue {
            rule_id: entry.rule.id.clone(),
            code: issue.code.clone(),
            severity: issue.severity,
            detail: issue.detail.clone(),
        }));
        rules.push(EvaluatedRule {
            id: entry.rule.id.clone(),
            origin: entry.origin,
            rules_version: if entry.origin == RuleOrigin::Local {
                String::new()
            } else {
                set.document.version.clone()
            },
            reason: entry.rule.reason.clone(),
            confidence: entry.rule.confidence,
            source: entry.rule.source.clone(),
            applied: changes.iter().any(|change| change.applied) || !added_prerequisites.is_empty(),
            changes,
            prerequisites: added_prerequisites,
            known_issues: entry.rule.known_issues.clone(),
        });
    }

    RuleEvaluation {
        options,
        extra_args,
        rules,
        prerequisites: prerequisites.into_iter().collect(),
        issues,
    }
}

fn apply_rule(
    rule: &CompatRule,
    options: &mut LaunchOptions,
    extra_args: &mut Vec<String>,
    claimed: &mut BTreeSet<String>,
) -> (Vec<RuleChange>, Vec<String>) {
    let mut changes = Vec::new();
    let mut prerequisites = Vec::new();

    if let Some(mode) = rule.plan.mode {
        let field = "mode".to_string();
        let value = mode.as_str().to_string();
        if claimed.contains(&field) {
            changes.push(RuleChange::skipped(&field, value, "rule-conflict"));
        } else if options.mode != LaunchMode::Auto {
            changes.push(RuleChange::skipped(&field, value, "user-choice"));
        } else {
            claimed.insert(field.clone());
            options.mode = mode;
            changes.push(RuleChange::applied(field, value));
        }
    }

    for argument in &rule.plan.args {
        if extra_args.contains(argument) {
            changes.push(RuleChange::skipped("args", argument.clone(), "duplicate"));
        } else {
            extra_args.push(argument.clone());
            changes.push(RuleChange::applied("args", argument.clone()));
        }
    }

    for (key, value) in &rule.plan.env {
        let field = format!("env.{key}");
        if claimed.contains(&field) {
            changes.push(RuleChange::skipped(&field, value.clone(), "rule-conflict"));
        } else if options.env.contains_key(key) {
            changes.push(RuleChange::skipped(&field, value.clone(), "user-choice"));
        } else {
            claimed.insert(field.clone());
            options.env.insert(key.clone(), value.clone());
            changes.push(RuleChange::applied(field, value.clone()));
        }
    }

    if !rule.plan.winetricks.is_empty() {
        let value = rule.plan.winetricks.join(", ");
        if claimed.contains("winetricks") {
            changes.push(RuleChange::skipped("winetricks", value, "rule-conflict"));
        } else if !options.winetricks_verbs.is_empty() {
            changes.push(RuleChange::skipped("winetricks", value, "user-choice"));
        } else {
            claimed.insert("winetricks".to_string());
            options.winetricks_verbs = rule.plan.winetricks.clone();
            changes.push(RuleChange::applied("winetricks", value));
        }
    }

    if let Some(app_id) = rule.plan.steam_app_id {
        let field = "steamAppId".to_string();
        let value = app_id.to_string();
        if claimed.contains(&field) {
            changes.push(RuleChange::skipped(&field, value, "rule-conflict"));
        } else if options.steam_app_id.is_some() {
            changes.push(RuleChange::skipped(&field, value, "user-choice"));
        } else {
            claimed.insert(field.clone());
            options.steam_app_id = Some(app_id);
            changes.push(RuleChange::applied(field, value));
        }
    }

    if let Some(display) = &rule.plan.display {
        // Validation guarantees a complete Gamescope profile, so this is an
        // all-or-nothing change: a partial apply would be an invalid profile.
        let mut patch = vec![("display.mode".to_string(), "gamescope".to_string())];
        if let Some(width) = display.width {
            patch.push(("display.width".to_string(), width.to_string()));
        }
        if let Some(height) = display.height {
            patch.push(("display.height".to_string(), height.to_string()));
        }
        if let Some(refresh) = display.refresh {
            patch.push(("display.refresh".to_string(), refresh.to_string()));
        }
        if let Some(filter) = &display.filter {
            patch.push(("display.filter".to_string(), filter.clone()));
        }
        if let Some(adaptive_sync) = display.adaptive_sync {
            patch.push((
                "display.adaptiveSync".to_string(),
                if adaptive_sync { "on" } else { "off" }.to_string(),
            ));
        }
        if let Some(hdr) = display.hdr {
            patch.push((
                "display.hdr".to_string(),
                if hdr { "on" } else { "off" }.to_string(),
            ));
        }

        if claimed.contains("display.mode") {
            changes.extend(
                patch
                    .into_iter()
                    .map(|(field, value)| RuleChange::skipped(field, value, "rule-conflict")),
            );
        } else if options.display != DisplayProfile::default() {
            changes.extend(
                patch
                    .into_iter()
                    .map(|(field, value)| RuleChange::skipped(field, value, "user-choice")),
            );
        } else {
            options.display = DisplayProfile {
                mode: DisplayMode::Gamescope,
                width: display.width.unwrap_or_default(),
                height: display.height.unwrap_or_default(),
                refresh: display.refresh.unwrap_or_default(),
                filter: display.filter.clone().unwrap_or_default(),
                adaptive_sync: display.adaptive_sync.unwrap_or(false),
                hdr: display.hdr.unwrap_or(false),
            };
            for (field, value) in patch {
                claimed.insert(field.clone());
                changes.push(RuleChange::applied(field, value));
            }
        }
    }

    for prerequisite in &rule.prerequisites {
        if !prerequisites.contains(prerequisite) {
            prerequisites.push(prerequisite.clone());
        }
    }

    (changes, prerequisites)
}

/* -------------------------------------------------------------- fetching */

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RulesState {
    checked_at: Option<String>,
    fetched_at: Option<String>,
    version: Option<String>,
    error: Option<String>,
}

/// The provider status shown in Settings and used by the update path.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatRulesStatus {
    pub schema: u32,
    pub supported_schema: u32,
    /// Active database label (`2026.11`).
    pub version: String,
    /// `shipped` or `cached`.
    pub origin: RuleOrigin,
    pub rule_count: usize,
    pub local_rule_count: usize,
    /// True when a layer was skipped; the active rules are still usable.
    pub degraded: bool,
    pub notes: Vec<String>,
    pub cache_path: String,
    pub local_path: String,
    pub signature_configured: bool,
    pub last_checked_at: Option<String>,
    pub fetched_at: Option<String>,
    /// Last update failure, in English; `None` after a successful check.
    pub message: Option<String>,
}

impl CompatRulesStatus {
    fn from_set(data_dir: &Path, set: &RuleSet, signature_configured: bool) -> Self {
        let state = read_state(data_dir);
        Self {
            schema: set.document.schema,
            supported_schema: SCHEMA_VERSION,
            version: set.document.version.clone(),
            origin: set.origin,
            rule_count: set.entries.len(),
            local_rule_count: set.local_rule_count(),
            degraded: set.degraded,
            notes: set.notes.clone(),
            cache_path: data_dir.join(CACHE_FILE).to_string_lossy().to_string(),
            local_path: data_dir.join(LOCAL_FILE).to_string_lossy().to_string(),
            signature_configured,
            last_checked_at: state.checked_at,
            fetched_at: state.fetched_at,
            message: state.error,
        }
    }
}

/// One game's rules, for the "why" panel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompatRulesReport {
    pub game_id: i64,
    pub version: String,
    pub origin: RuleOrigin,
    pub local_rule_count: usize,
    pub degraded: bool,
    pub notes: Vec<String>,
    pub evaluation: RuleEvaluation,
}

fn read_state(data_dir: &Path) -> RulesState {
    std::fs::read_to_string(data_dir.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_state(data_dir: &Path, state: &RulesState) {
    let path = data_dir.join(STATE_FILE);
    match serde_json::to_vec_pretty(state) {
        Ok(bytes) => {
            if let Err(error) = write_atomic(&path, &bytes) {
                tracing::debug!(error = %error, path = %path.display(), "could not write the compat rules state");
            }
        }
        Err(error) => tracing::debug!(error = %error, "could not encode the compat rules state"),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Whether the automatic check should run now. An explicit update ignores it.
fn should_refresh(state: &RulesState, now: chrono::DateTime<chrono::Utc>, force: bool) -> bool {
    if force {
        return true;
    }
    let Some(checked) = state.checked_at.as_deref() else {
        return true;
    };
    match chrono::DateTime::parse_from_rfc3339(checked) {
        Ok(parsed) => {
            now.signed_duration_since(parsed.with_timezone(&chrono::Utc))
                >= chrono::Duration::hours(AUTO_REFRESH_HOURS)
        }
        Err(_) => true,
    }
}

/// Download, verify and cache the rules. Never returns a network error: the
/// caller renders the status, and a failure leaves the current rules in place.
///
/// This is the only function that touches the network. The startup path spawns
/// it detached (`spawn_startup_refresh`); a launch never calls it.
pub async fn update(app: &AppHandle, force: bool) -> CompatRulesStatus {
    // Two startup paths may race (the detached refresh and an explicit call).
    // The lock makes the throttle and the cache write a single decision.
    static UPDATE_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _guard = UPDATE_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    let (data_dir, http) = match app.try_state::<AppState>() {
        Some(state) => (state.app_data.clone(), state.http.clone()),
        None => {
            return CompatRulesStatus::from_set(
                Path::new(""),
                &load_from(Path::new(""), None),
                false,
            )
        }
    };
    let public_key = configured_public_key(app);
    let signature_configured = public_key.is_some();
    let mut stored = read_state(&data_dir);

    // Source builds have no release key. There is nothing to verify a download
    // with, so do not fetch and do not record an error: the shipped rules are
    // the whole database for this build.
    if public_key.is_none() {
        let set = load_from(&data_dir, None);
        return CompatRulesStatus::from_set(&data_dir, &set, false);
    }

    let now = chrono::Utc::now();
    if !should_refresh(&stored, now, force) {
        let set = load_from(&data_dir, public_key.as_deref());
        return CompatRulesStatus::from_set(&data_dir, &set, signature_configured);
    }
    stored.checked_at = Some(crate::util::now_rfc3339());

    let outcome = fetch_and_store(&http, &data_dir, public_key.as_deref(), &stored).await;
    match outcome {
        Ok(version) => {
            stored.error = None;
            stored.version = Some(version.clone());
            stored.fetched_at = Some(crate::util::now_rfc3339());
            tracing::info!(version = %version, "compatibility rules updated");
        }
        Err(error) => {
            log_once(
                "update-failed",
                &format!("compatibility rules update failed: {error}"),
            );
            stored.error = Some(error);
        }
    }
    write_state(&data_dir, &stored);
    let set = load_from(&data_dir, public_key.as_deref());
    CompatRulesStatus::from_set(&data_dir, &set, signature_configured)
}

async fn fetch_and_store(
    http: &reqwest::Client,
    data_dir: &Path,
    public_key: Option<&str>,
    stored: &RulesState,
) -> Result<String, String> {
    let Some(public_key) = public_key else {
        return Err("this build has no signing key; the shipped rules stay in use".to_string());
    };

    let rules = fetch(http, RULES_URL).await?;
    let signature = fetch_text(http, RULES_SIGNATURE_URL).await?;
    verify_signature(&rules, &signature, public_key)?;

    let text = String::from_utf8(rules).map_err(|_| "the rules file is not UTF-8".to_string())?;
    let document = parse_rules(&text, RuleOrigin::Cached)
        .map_err(|errors| format!("the rules file is invalid: {}", errors.join("; ")))?;

    if let Some(previous) = stored.version.as_deref() {
        let previous_key = parse_version_key(previous).unwrap_or_default();
        let next_key = parse_version_key(&document.version).unwrap_or_default();
        if next_key < previous_key {
            return Err(format!(
                "the downloaded rules ({}) are older than the cached copy ({previous})",
                document.version
            ));
        }
    }

    write_atomic(&data_dir.join(CACHE_SIGNATURE_FILE), signature.as_bytes())
        .map_err(|error| format!("could not cache the signature: {error}"))?;
    write_atomic(&data_dir.join(CACHE_FILE), text.as_bytes())
        .map_err(|error| format!("could not cache the rules: {error}"))?;
    Ok(document.version)
}

async fn fetch(http: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = http
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|error| format!("{url} is unreachable: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} answered HTTP {}", response.status()));
    }
    if let Some(length) = response.content_length() {
        if length as usize > MAX_RULES_BYTES {
            return Err(format!("{url} is larger than {MAX_RULES_BYTES} bytes"));
        }
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("{url} could not be read: {error}"))?;
        if body.len() + chunk.len() > MAX_RULES_BYTES {
            return Err(format!("{url} is larger than {MAX_RULES_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<String, String> {
    let bytes = fetch(http, url).await?;
    String::from_utf8(bytes).map_err(|_| format!("{url} is not UTF-8"))
}

/// Background refresh, detached from the startup path. A no-op while the 24 h
/// throttle is active or when this build has no signing key.
pub fn spawn_startup_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let status = update(&app, false).await;
        if let Some(message) = status.message.as_deref() {
            tracing::debug!(message = %message, "compatibility rules are unchanged");
        }
    });
}

/// Log a degradation once per process, never on every panel poll.
fn log_once(key: &str, message: &str) {
    static LOGGED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let logged = LOGGED.get_or_init(|| Mutex::new(BTreeSet::new()));
    let first = match logged.lock() {
        Ok(mut set) => set.insert(key.to_string()),
        Err(_) => true,
    };
    if first {
        tracing::warn!(rules = key, "compatibility rules: {message}");
    }
}

/* --------------------------------------------------------------- commands */

fn app_data(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map_err(|error| format!("The app data folder is unavailable: {error}"))
}

fn load_for_app(app: &AppHandle) -> RuleSet {
    match app_data(app) {
        Ok(data_dir) => load_from(&data_dir, configured_public_key(app).as_deref()),
        Err(error) => {
            log_once("app-data", &error);
            load_from(Path::new(""), None)
        }
    }
}

/// Provider status for the rules database.
#[tauri::command]
pub fn compat_rules_status(app: AppHandle) -> CompatRulesStatus {
    let set = load_for_app(&app);
    let data_dir = app_data(&app).unwrap_or_default();
    CompatRulesStatus::from_set(&data_dir, &set, configured_public_key(&app).is_some())
}

/// The rules that match one installed game, with reasons. Read-only. The
/// executable is hashed only when a rule asks for `exe_sha256`; the work runs
/// on a blocking thread so a large executable never stalls the window.
#[tauri::command]
pub async fn compat_rules_report(
    app: AppHandle,
    game_id: i64,
) -> Result<CompatRulesReport, String> {
    tauri::async_runtime::spawn_blocking(move || report_for_game(&app, game_id))
        .await
        .map_err(|error| format!("The compatibility rules report failed: {error}"))?
}

fn report_for_game(app: &AppHandle, game_id: i64) -> Result<CompatRulesReport, String> {
    let state = app.state::<AppState>();
    let installed = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_installed(&connection, game_id)?
    };
    let Some(installed) = installed else {
        return Err(format!("Game {game_id} is not installed on this device."));
    };
    let base = {
        let connection = state
            .db
            .lock()
            .map_err(|_| "The local database is unavailable; restart Ludex.".to_string())?;
        db::get_launch_options(&connection, game_id)?
    };

    let set = match app_data(app) {
        Ok(data_dir) => load_from(&data_dir, configured_public_key(app).as_deref()),
        Err(error) => {
            log_once("app-data", &error);
            load_from(Path::new(""), None)
        }
    };

    let install_path = PathBuf::from(&installed.install_path);
    let executable = installed.executable.as_deref().and_then(|relative| {
        let candidate = install_path.join(relative);
        candidate.starts_with(&install_path).then_some(candidate)
    });
    let evaluation = plan_for_launch(&set, game_id, &base, executable.as_deref(), None, None);
    Ok(CompatRulesReport {
        game_id,
        version: set.version().to_string(),
        origin: set.origin,
        local_rule_count: set.local_rule_count(),
        degraded: set.degraded,
        notes: set.notes.clone(),
        evaluation,
    })
}

/// Fetch the signed rules now and return the provider status. A failure is
/// reported in `message`, never as an error: the active rules keep working.
#[tauri::command]
pub async fn compat_rules_update(
    app: AppHandle,
    force: Option<bool>,
) -> Result<CompatRulesStatus, String> {
    Ok(update(&app, force.unwrap_or(false)).await)
}

/// Narrow interface for the launch planner (#297): evaluate the database
/// against one game without going through a Tauri command.
///
/// The caller loads the [`RuleSet`] once and passes the facts it already
/// probed (`executable`, `engine`, `gpu_hint`). The returned `options` is a
/// complete replacement for the stored plan, and `extra_args` is appended after
/// the user's startup command.
#[must_use]
pub fn plan_for_launch(
    set: &RuleSet,
    game_id: i64,
    base: &LaunchOptions,
    executable: Option<&Path>,
    engine: Option<&str>,
    gpu_hint: Option<&str>,
) -> RuleEvaluation {
    let kind = executable
        .map(executable_kind)
        .unwrap_or(ExecutableKind::Unknown);
    let wants_hash = set
        .entries()
        .iter()
        .any(|entry| entry.rule.matches.exe_sha256.is_some());
    let input = MatchInput {
        exe_sha256: executable
            .filter(|_| wants_hash)
            .and_then(executable_sha256),
        engine: engine.map(str::to_string),
        gpu_vendor: gpu_hint
            .map(GpuVendor::from_hint)
            .unwrap_or(GpuVendor::Unknown),
        executable: kind,
        ..MatchInput::for_host(game_id)
    };
    evaluate(set, &input, base)
}

/* ------------------------------------------------------------------ tests */

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-compat-rules-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn base_options() -> LaunchOptions {
        LaunchOptions::default()
    }

    fn shipped() -> RuleSet {
        load_from(&temp_dir("empty"), None)
    }

    fn linux_windows_input() -> MatchInput {
        MatchInput {
            game_id: 7,
            exe_sha256: None,
            engine: None,
            platform: RulePlatform::Linux,
            arch: "x86_64".to_string(),
            gpu_vendor: GpuVendor::Unknown,
            executable: ExecutableKind::Windows,
        }
    }

    fn document(version: &str) -> String {
        format!(
            r#"
schema = 1
version = "{version}"
updated = "2026-11-19"

[[rules]]
id = "test-rule"
reason = "A test reason."
confidence = "certain"
source = "test"

[rules.match]
platform = "linux"
executable = "windows"

[[rules.known_issues]]
code = "test-issue"
severity = "info"
detail = "A test issue."
"#
        )
    }

    /* -- parsing and validation -- */

    #[test]
    fn the_shipped_file_is_valid_and_current() {
        let document = parse_rules(SHIPPED_RULES_TOML, RuleOrigin::Shipped)
            .unwrap_or_else(|errors| panic!("shipped rules are invalid: {errors:?}"));
        assert_eq!(document.schema, SCHEMA_VERSION);
        assert!(!document.rules.is_empty());
        assert!(
            parse_version_key(&document.version).is_some(),
            "the shipped version {:?} is not YYYY.MM",
            document.version
        );
        assert!(!document.updated.is_empty());
        assert!(!document.description.trim().is_empty());
        // Every shipped rule is justified in the panel.
        for rule in &document.rules {
            assert!(!rule.reason.trim().is_empty(), "{} has no reason", rule.id);
            assert!(!rule.source.trim().is_empty(), "{} has no source", rule.id);
        }
    }

    #[test]
    fn a_schema_from_the_future_is_refused() {
        let text = document("2026.12").replace("schema = 1", "schema = 2");
        let errors = parse_rules(&text, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("schema 2")),
            "{errors:?}"
        );
    }

    #[test]
    fn parsing_reports_every_problem() {
        let text = r#"
schema = 1
version = "not-a-version"

[[rules]]
id = "Bad_Id"
reason = ""
confidence = "certain"
source = ""

[rules.match]

[rules.plan]
env = { "PATH" = "/tmp" }
steam_app_id = 0
"#;
        let errors = parse_rules(text, RuleOrigin::Shipped).unwrap_err();
        let joined = errors.join("\n");
        assert!(joined.contains("version"), "{joined}");
        assert!(joined.contains("id must match"), "{joined}");
        assert!(joined.contains("reason is empty"), "{joined}");
        assert!(joined.contains("source is empty"), "{joined}");
        assert!(joined.contains("match is empty"), "{joined}");
        assert!(joined.contains("PATH"), "{joined}");
        assert!(joined.contains("steam_app_id"), "{joined}");
    }

    #[test]
    fn unknown_plan_keys_are_rejected_at_parse_time() {
        let text = document("2026.12") + "\n[rules.plan]\nworking_dir = \"/tmp\"\n";
        let errors = parse_rules(&text, RuleOrigin::Shipped).unwrap_err();
        assert!(errors[0].contains("unknown field"), "{errors:?}");
    }

    #[test]
    fn shipped_rules_cannot_match_a_local_game_id() {
        let text = document("2026.12")
            .replace("platform = \"linux\"", "game_id = 42\nplatform = \"linux\"");
        let errors = parse_rules(&text, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("local game_id")),
            "{errors:?}"
        );
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let text = document("2026.12");
        let doubled = format!(
            "{text}\n{}",
            text.split("[[rules]]")
                .nth(1)
                .map(|tail| format!("[[rules]]{tail}"))
                .unwrap()
        );
        let errors = parse_rules(&doubled, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("duplicate id")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_rule_that_does_nothing_is_refused() {
        let text = r#"
schema = 1
version = "2026.12"

[[rules]]
id = "empty-rule"
reason = "Nothing happens."
confidence = "guess"
source = "test"

[rules.match]
platform = "linux"
"#;
        let errors = parse_rules(text, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("must change the plan")),
            "{errors:?}"
        );
    }

    #[test]
    fn prerequisites_must_be_known() {
        let text = document("2026.12").replace(
            "[rules.match]",
            "prerequisites = [\"metal\"]\n[rules.match]",
        );
        let errors = parse_rules(&text, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("\"metal\"")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_partial_display_patch_is_refused() {
        let text =
            document("2026.12") + "\n[rules.plan.display]\nmode = \"gamescope\"\nwidth = 1920\n";
        let errors = parse_rules(&text, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("display.height")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_desktop_display_patch_is_refused() {
        let text = document("2026.12") + "\n[rules.plan.display]\nhdr = true\n";
        let errors = parse_rules(&text, RuleOrigin::Shipped).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.contains("mode = \"gamescope\"")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_complete_display_patch_is_accepted() {
        let text = document("2026.12")
            + "\n[rules.plan.display]\nmode = \"gamescope\"\nwidth = 1920\nheight = 1080\nhdr = true\n";
        let parsed = parse_rules(&text, RuleOrigin::Shipped).unwrap();
        assert_eq!(
            parsed.rules[0].plan.display.as_ref().unwrap().width,
            Some(1920)
        );
    }

    #[test]
    fn local_rules_may_use_a_game_id_and_skip_the_source() {
        let text = r#"
[[rules]]
id = "local-learning"
reason = "This game needed the prefix reset on this machine."
confidence = "guess"

[rules.match]
game_id = 42
platform = "linux"

[rules.plan]
env = { WINEDLLOVERRIDES = "winemenubuilder.exe=d" }
"#;
        let rules = parse_local_rules(text).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].matches.game_id, Some(42));
    }

    #[test]
    fn local_rules_never_override_reserved_env() {
        let text = r#"
[[rules]]
id = "local-reserved"
reason = "Test."
confidence = "guess"

[rules.match]
game_id = 42

[rules.plan]
env = { PATH = "/tmp/bin" }
"#;
        let errors = parse_local_rules(text).unwrap_err();
        assert!(
            errors.iter().any(|error| error.contains("PATH")),
            "{errors:?}"
        );
    }

    #[test]
    fn version_keys_order_across_patches() {
        assert!(parse_version_key("2026.12").unwrap() > parse_version_key("2026.11").unwrap());
        assert!(parse_version_key("2026.11.2").unwrap() > parse_version_key("2026.11").unwrap());
        assert!(parse_version_key("2026.10").unwrap() < parse_version_key("2026.11.9").unwrap());
        assert!(parse_version_key("2026-11").is_none());
        assert!(parse_version_key("2026.00").is_none());
        assert!(parse_version_key("2026.11.1.2").is_none());
    }

    /* -- layers and precedence -- */

    #[test]
    fn a_missing_local_file_is_not_a_degradation() {
        let set = shipped();
        assert!(!set.degraded);
        assert_eq!(set.local_rule_count(), 0);
        assert_eq!(set.origin, RuleOrigin::Shipped);
        assert_eq!(set.version(), "2026.11");
    }

    #[test]
    fn a_local_rule_with_the_same_id_replaces_the_shipped_one() {
        let dir = temp_dir("replace");
        fs::write(
            dir.join(LOCAL_FILE),
            r#"
[[rules]]
id = "linux-windows-exe-needs-a-compat-tool"
reason = "Learned on this machine."
confidence = "guess"

[rules.match]
game_id = 3

[rules.plan]
mode = "native"
"#,
        )
        .unwrap();
        let set = load_from(&dir, None);
        assert_eq!(set.local_rule_count(), 1);
        let entries: Vec<&CompatRule> = set.entries().iter().map(|entry| &entry.rule).collect();
        assert_eq!(
            entries
                .iter()
                .filter(|rule| rule.id == "linux-windows-exe-needs-a-compat-tool")
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .find(|rule| rule.id == "linux-windows-exe-needs-a-compat-tool")
                .unwrap()
                .reason,
            "Learned on this machine."
        );
        assert_eq!(set.entries()[0].origin, RuleOrigin::Local);
    }

    #[test]
    fn a_broken_local_file_degrades_to_the_shipped_rules() {
        let dir = temp_dir("broken-local");
        fs::write(dir.join(LOCAL_FILE), "this is not toml [").unwrap();
        let set = load_from(&dir, None);
        assert!(set.degraded);
        assert_eq!(set.local_rule_count(), 0);
        assert_eq!(set.origin, RuleOrigin::Shipped);
        assert!(!set.entries().is_empty());
        assert!(set.notes.iter().any(|note| note.contains("ignored")));
    }

    #[test]
    fn a_cache_without_a_signing_key_is_ignored() {
        let dir = temp_dir("cache-no-key");
        fs::write(dir.join(CACHE_FILE), document("2099.01")).unwrap();
        fs::write(dir.join(CACHE_SIGNATURE_FILE), "not a signature").unwrap();
        let set = load_from(&dir, None);
        assert!(set.degraded);
        assert_eq!(set.origin, RuleOrigin::Shipped);
        assert!(set.notes.iter().any(|note| note.contains("signing key")));
    }

    #[test]
    fn a_tampered_cache_is_ignored() {
        let dir = temp_dir("cache-tampered");
        fs::write(dir.join(CACHE_FILE), document("2099.01")).unwrap();
        fs::write(dir.join(CACHE_SIGNATURE_FILE), "bogus").unwrap();
        let set = load_from(&dir, Some(&test_public_key()));
        assert!(set.degraded);
        assert_eq!(set.origin, RuleOrigin::Shipped);
        assert!(set.notes.iter().any(|note| note.contains("signature")));
    }

    #[test]
    fn a_signed_cache_newer_than_the_build_is_used() {
        let dir = temp_dir("cache-signed");
        fs::write(dir.join(CACHE_FILE), FIXTURE_DOCUMENT).unwrap();
        fs::write(dir.join(CACHE_SIGNATURE_FILE), FIXTURE_SIGNATURE).unwrap();
        let set = load_from(&dir, Some(&test_public_key()));
        assert_eq!(set.origin, RuleOrigin::Cached);
        assert_eq!(set.version(), "2099.01");
        assert_eq!(set.entries.len(), 1);
        assert_eq!(set.entries[0].rule.id, "cached-test-rule");
    }

    #[test]
    fn an_older_cache_never_downgrades_the_shipped_rules() {
        let dir = temp_dir("cache-older");
        // Valid signature, valid schema, but an older label than the build.
        fs::write(dir.join(CACHE_FILE), OLD_FIXTURE_DOCUMENT).unwrap();
        fs::write(dir.join(CACHE_SIGNATURE_FILE), OLD_FIXTURE_SIGNATURE).unwrap();
        let set = load_from(&dir, Some(&test_public_key()));
        assert_eq!(set.origin, RuleOrigin::Shipped);
        assert!(
            set.notes.iter().any(|note| note.contains("older")),
            "{:?}",
            set.notes
        );
    }

    /* -- signature -- */

    /// Minisign vector from the `minisign-verify` documentation (signs `test`).
    const DOC_PUBLIC_KEY: &str = "untrusted comment: minisign public key: 1F2B...\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const DOC_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

    /// A throwaway key pair, generated for this test only. The private key is
    /// not in the repository; the document below is signed with it so the
    /// signed-cache path is exercised end to end.
    const FIXTURE_PUBLIC_KEY: &str = "untrusted comment: minisign public key 2D93C8A417540552\nRWRSBVQXpMiTLXjTP6SrZQleiOCqjMsoB+mX7oYTEOjWnty+4wlpM4AX";
    const FIXTURE_DOCUMENT: &str = r#"schema = 1
version = "2099.01"
updated = "2026-11-19"

[[rules]]
id = "cached-test-rule"
reason = "A signed fixture rule."
confidence = "certain"
source = "test fixture"

[rules.match]
platform = "linux"

[[rules.known_issues]]
code = "fixture-only"
severity = "info"
detail = "A fixture, never shipped."
"#;
    const FIXTURE_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRURSBVQXpMiTLat6tNE4hgZYCwPDRdtWV4FuslnmNUMgLf5TI/+UH5v+6Jtdj/GWbHP+2DYG/cDAwu0CXkjlhUrJPqlq2ovzpQo=\ntrusted comment: timestamp:1795119148\tfile:compat-test.toml\thashed\n4Sq+QOGQblzMHStKRyBTSDfsjjZ1DD++k+to9AWsNYNUpSkakgg6NTikroLGCsJPmtgQNsJauyrZvUS0bKioCg==";
    const OLD_FIXTURE_DOCUMENT: &str = r#"schema = 1
version = "2020.01"
updated = "2026-11-19"

[[rules]]
id = "cached-old-rule"
reason = "An older signed fixture rule."
confidence = "certain"
source = "test fixture"

[rules.match]
platform = "linux"

[[rules.known_issues]]
code = "fixture-old"
severity = "info"
detail = "An older fixture."
"#;
    const OLD_FIXTURE_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRURSBVQXpMiTLf+h9KFox/22PTnTOvFUaStjhougHdPSVPBYGRndJZdBpxN6vUJmoXTmmmjm1+ltej9hXSlxuIqd+BpbrMaTuQo=\ntrusted comment: timestamp:1795119188\tfile:compat-old.toml\thashed\nuqm6vTBn12NW3lxKmQO8GR1gS/VpT6McT/BUzjIdgmgmYz1PIacQgQ2WWAQL+EbCtMnZmWx8PuGvTOa+inmqCg==";

    fn test_public_key() -> String {
        FIXTURE_PUBLIC_KEY.to_string()
    }

    #[test]
    fn a_valid_signature_verifies() {
        verify_signature(b"test", DOC_SIGNATURE, DOC_PUBLIC_KEY).unwrap();
        verify_signature(
            FIXTURE_DOCUMENT.as_bytes(),
            FIXTURE_SIGNATURE,
            FIXTURE_PUBLIC_KEY,
        )
        .unwrap();
    }

    #[test]
    fn a_modified_payload_is_rejected() {
        assert!(verify_signature(b"test!", DOC_SIGNATURE, DOC_PUBLIC_KEY).is_err());
        let mut altered = FIXTURE_DOCUMENT.to_string();
        altered.push('\n');
        assert!(
            verify_signature(altered.as_bytes(), FIXTURE_SIGNATURE, FIXTURE_PUBLIC_KEY).is_err()
        );
    }

    #[test]
    fn a_garbage_signature_is_rejected() {
        assert!(verify_signature(b"test", "nope", DOC_PUBLIC_KEY).is_err());
    }

    #[test]
    fn the_base64_signature_form_tauri_writes_verifies() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(FIXTURE_SIGNATURE);
        verify_signature(FIXTURE_DOCUMENT.as_bytes(), &encoded, FIXTURE_PUBLIC_KEY).unwrap();
    }

    #[test]
    fn the_tauri_pubkey_decodes_to_the_same_key() {
        // The updater and the rules share this key; tauri.conf.json keeps it
        // base64-encoded, exactly like this test builds it.
        let text = "untrusted comment: minisign public key: 67DF1F7D6F8425D8\nRWTYJYRvfR/fZ7gvqJswoVBRjOGygHbRCgCkyUEF5pQFLYpiEnwDthsC\n";
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        assert_eq!(tauri_public_key(&encoded).as_deref(), Some(text));
    }

    #[test]
    fn an_unconfigured_pubkey_is_reported_as_absent() {
        assert!(tauri_public_key("").is_none());
        assert!(tauri_public_key("!!").is_none());
    }

    /* -- matching and applying -- */

    #[test]
    fn every_condition_must_hold() {
        let text = r#"
schema = 1
version = "2026.12"

[[rules]]
id = "nvidia-unity-rule"
reason = "Test."
confidence = "likely"
source = "test"

[rules.match]
platform = "linux"
executable = "windows"
engine = "unity"
gpu_vendor = "nvidia"

[rules.plan]
mode = "umu"
"#;
        let parsed = parse_rules(text, RuleOrigin::Shipped).unwrap();
        let rule = &parsed.rules[0];
        let mut input = linux_windows_input();
        assert!(
            !rule_matches(rule, &input),
            "no engine and no vendor must not match"
        );
        input.engine = Some("Unity".to_string());
        assert!(!rule_matches(rule, &input), "the vendor is still unknown");
        input.gpu_vendor = GpuVendor::Nvidia;
        assert!(rule_matches(rule, &input));
        input.executable = ExecutableKind::Native;
        assert!(!rule_matches(rule, &input));
    }

    #[test]
    fn the_shipped_rules_match_the_expected_hosts() {
        let set = shipped();
        let evaluation = evaluate(&set, &linux_windows_input(), &base_options());
        assert!(evaluation
            .rules
            .iter()
            .any(|rule| rule.id == "linux-windows-exe-needs-a-compat-tool"));
        assert!(!evaluation
            .rules
            .iter()
            .any(|rule| rule.id == "linux-native-executable-runs-directly"));
        assert!(evaluation
            .issues
            .iter()
            .any(|issue| issue.code == "compat-layer-required"
                && issue.severity == IssueSeverity::Info));
    }

    #[test]
    fn a_rule_fills_an_automatic_mode_but_not_a_user_choice() {
        let set = shipped();
        let input = MatchInput {
            executable: ExecutableKind::Native,
            ..linux_windows_input()
        };
        let evaluation = evaluate(&set, &input, &base_options());
        assert_eq!(evaluation.options.mode, LaunchMode::Native);
        assert!(evaluation.rules.iter().any(|rule| rule.applied));

        let user = LaunchOptions {
            mode: LaunchMode::Umu,
            ..LaunchOptions::default()
        };
        let evaluation = evaluate(&set, &input, &user);
        assert_eq!(evaluation.options.mode, LaunchMode::Umu);
        let rule = evaluation
            .rules
            .iter()
            .find(|rule| rule.id == "linux-native-executable-runs-directly")
            .unwrap();
        assert!(!rule.applied);
        assert_eq!(rule.changes[0].skipped.as_deref(), Some("user-choice"));
    }

    #[test]
    fn rule_overrides_reach_every_supported_plan_field() {
        let text = r#"
schema = 1
version = "2026.12"

[[rules]]
id = "everything-rule"
reason = "Test."
confidence = "guess"
source = "test"
prerequisites = ["vcrun", "dotnet"]

[rules.match]
game_id = 7

[rules.plan]
mode = "proton"
args = ["-windowed"]
winetricks = ["mfplat"]
steam_app_id = 480

[rules.plan.env]
PROTON_ENABLE_NVAPI = "1"

[rules.plan.display]
mode = "gamescope"
width = 2560
height = 1440
refresh = 120
filter = "fsr"
adaptive_sync = true
hdr = true

[[rules.known_issues]]
code = "test-issue"
severity = "warning"
detail = "Only a test."
"#;
        let rules = parse_local_rules(text).unwrap();
        let set = RuleSet {
            document: RulesDocument {
                schema: 1,
                version: "local".to_string(),
                updated: String::new(),
                description: String::new(),
                rules: Vec::new(),
            },
            origin: RuleOrigin::Shipped,
            entries: vec![RuleEntry {
                origin: RuleOrigin::Local,
                rule: rules[0].clone(),
            }],
            notes: Vec::new(),
            degraded: false,
        };
        let input = MatchInput {
            executable: ExecutableKind::Windows,
            ..linux_windows_input()
        };
        let evaluation = evaluate(&set, &input, &base_options());
        assert_eq!(evaluation.options.mode, LaunchMode::Proton);
        assert_eq!(evaluation.extra_args, vec!["-windowed".to_string()]);
        assert_eq!(
            evaluation
                .options
                .env
                .get("PROTON_ENABLE_NVAPI")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            evaluation.options.winetricks_verbs,
            vec!["mfplat".to_string()]
        );
        assert_eq!(evaluation.options.steam_app_id, Some(480));
        assert_eq!(evaluation.options.display.mode, DisplayMode::Gamescope);
        assert_eq!(evaluation.options.display.width, 2560);
        assert_eq!(evaluation.options.display.height, 1440);
        assert_eq!(evaluation.options.display.refresh, 120);
        assert_eq!(evaluation.options.display.filter, "fsr");
        assert!(evaluation.options.display.adaptive_sync);
        assert!(evaluation.options.display.hdr);
        // The set is sorted, so the report order is deterministic.
        assert_eq!(
            evaluation.prerequisites,
            vec!["dotnet".to_string(), "vcrun".to_string()]
        );
        assert_eq!(evaluation.issues.len(), 1);
        assert!(evaluation.rules[0].applied);
    }

    #[test]
    fn a_local_rule_is_evaluated_first_and_wins_conflicts() {
        let shipped_rule: CompatRule = toml::from_str(
            r#"
id = "shipped-env"
reason = "Shipped."
confidence = "certain"
source = "test"

[match]
platform = "linux"

[plan.env]
PROTON_ENABLE_NVAPI = "0"
"#,
        )
        .unwrap();
        let local_rule: CompatRule = toml::from_str(
            r#"
id = "local-env"
reason = "Learned on this machine."
confidence = "guess"

[match]
game_id = 7

[plan.env]
PROTON_ENABLE_NVAPI = "1"
"#,
        )
        .unwrap();
        let document = RulesDocument {
            schema: SCHEMA_VERSION,
            version: "2026.12".to_string(),
            updated: String::new(),
            description: String::new(),
            rules: vec![shipped_rule],
        };
        let set = RuleSet {
            entries: merge_rules(&document, vec![local_rule]),
            document,
            origin: RuleOrigin::Shipped,
            notes: Vec::new(),
            degraded: false,
        };
        let evaluation = evaluate(&set, &linux_windows_input(), &base_options());
        assert_eq!(evaluation.rules[0].id, "local-env");
        assert_eq!(
            evaluation
                .options
                .env
                .get("PROTON_ENABLE_NVAPI")
                .map(String::as_str),
            Some("1")
        );
        let shipped = evaluation
            .rules
            .iter()
            .find(|rule| rule.id == "shipped-env")
            .unwrap();
        assert_eq!(shipped.changes[0].skipped.as_deref(), Some("rule-conflict"));
    }

    #[test]
    fn the_first_rule_wins_and_the_second_reports_a_conflict() {
        let dir = temp_dir("conflict-shipped");
        fs::write(
            dir.join(LOCAL_FILE),
            r#"
[[rules]]
id = "local-env-a"
reason = "Test."
confidence = "guess"

[rules.match]
game_id = 7

[rules.plan.env]
PROTON_ENABLE_NVAPI = "1"

[[rules]]
id = "local-env-b"
reason = "Test."
confidence = "guess"

[rules.match]
game_id = 7

[rules.plan.env]
PROTON_ENABLE_NVAPI = "0"
"#,
        )
        .unwrap();
        let set = load_from(&dir, None);
        let evaluation = evaluate(&set, &linux_windows_input(), &base_options());
        assert_eq!(
            evaluation
                .options
                .env
                .get("PROTON_ENABLE_NVAPI")
                .map(String::as_str),
            Some("1")
        );
        let second = evaluation
            .rules
            .iter()
            .find(|rule| rule.id == "local-env-b")
            .unwrap();
        assert_eq!(second.changes[0].skipped.as_deref(), Some("rule-conflict"));
    }

    #[test]
    fn a_default_display_profile_is_filled_once() {
        let text = r#"
schema = 1
version = "2026.12"

[[rules]]
id = "display-rule"
reason = "Test."
confidence = "guess"
source = "test"

[rules.match]
platform = "linux"

[rules.plan.display]
mode = "gamescope"
width = 1920
height = 1080
hdr = true
"#;
        let parsed = parse_rules(text, RuleOrigin::Shipped).unwrap();
        let set = RuleSet {
            document: parsed.clone(),
            origin: RuleOrigin::Shipped,
            entries: vec![RuleEntry {
                origin: RuleOrigin::Shipped,
                rule: parsed.rules[0].clone(),
            }],
            notes: Vec::new(),
            degraded: false,
        };
        let evaluation = evaluate(&set, &linux_windows_input(), &base_options());
        assert_eq!(evaluation.options.display.mode, DisplayMode::Gamescope);
        assert!(evaluation.options.display.hdr);

        let user = LaunchOptions {
            display: DisplayProfile {
                mode: DisplayMode::Gamescope,
                width: 1280,
                height: 720,
                ..DisplayProfile::default()
            },
            ..LaunchOptions::default()
        };
        let evaluation = evaluate(&set, &linux_windows_input(), &user);
        assert_eq!(evaluation.options.display.width, 1280);
        assert!(evaluation.rules[0]
            .changes
            .iter()
            .all(|change| !change.applied));
    }

    #[test]
    fn executable_kinds_follow_the_header() {
        let dir = temp_dir("kinds");
        let elf = dir.join("game");
        fs::write(&elf, b"\x7fELF\x02\x01\x01").unwrap();
        assert_eq!(executable_kind(&elf), ExecutableKind::Native);

        let stub = dir.join("game.exe");
        fs::write(&stub, b"MZ but no PE header").unwrap();
        assert_eq!(executable_kind(&stub), ExecutableKind::Unknown);

        let missing = dir.join("nope.exe");
        assert_eq!(executable_kind(&missing), ExecutableKind::Unknown);
    }

    #[test]
    fn hashing_a_missing_executable_is_none() {
        assert!(executable_sha256(Path::new("/nonexistent/game.exe")).is_none());
    }

    #[test]
    fn the_auto_refresh_throttle_honours_force_and_the_window() {
        let now = chrono::Utc::now();
        let mut state = RulesState::default();
        assert!(should_refresh(&state, now, false));
        state.checked_at = Some(crate::util::now_rfc3339());
        assert!(!should_refresh(&state, now, false));
        assert!(should_refresh(&state, now, true));
        state.checked_at = Some((now - chrono::Duration::hours(25)).to_rfc3339());
        assert!(should_refresh(&state, now, false));
        state.checked_at = Some("not a date".to_string());
        assert!(should_refresh(&state, now, false));
    }

    #[test]
    fn the_state_round_trips_through_json() {
        let dir = temp_dir("state");
        let state = RulesState {
            checked_at: Some("2026-11-19T10:00:00Z".to_string()),
            fetched_at: Some("2026-11-19T10:00:01Z".to_string()),
            version: Some("2026.11".to_string()),
            error: None,
        };
        write_state(&dir, &state);
        let loaded = read_state(&dir);
        assert_eq!(loaded.checked_at, state.checked_at);
        assert_eq!(loaded.version, state.version);
        assert!(loaded.error.is_none());
    }

    #[test]
    fn a_status_never_lies_about_the_active_version() {
        let dir = temp_dir("status");
        let set = load_from(&dir, None);
        let status = CompatRulesStatus::from_set(&dir, &set, false);
        assert_eq!(status.version, set.version());
        assert_eq!(status.schema, SCHEMA_VERSION);
        assert_eq!(status.supported_schema, SCHEMA_VERSION);
        assert_eq!(status.rule_count, set.entries.len());
        assert!(!status.signature_configured);
        assert!(!status.degraded);
    }

    #[test]
    fn gpu_vendor_hints_are_case_insensitive() {
        assert_eq!(
            GpuVendor::from_hint("NVIDIA GeForce RTX 4070"),
            GpuVendor::Nvidia
        );
        assert_eq!(GpuVendor::from_hint("AMD Radeon RX 7800"), GpuVendor::Amd);
        assert_eq!(GpuVendor::from_hint("ATI Technologies"), GpuVendor::Amd);
        assert_eq!(GpuVendor::from_hint("Intel Arc A770"), GpuVendor::Intel);
        assert_eq!(GpuVendor::from_hint(""), GpuVendor::Unknown);
    }
}
