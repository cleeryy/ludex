//! Shader preparation at install time (#293).
//!
//! The first minutes of a DXVK title on Linux are spent compiling pipelines
//! that the driver never saw before. The caches exist, but nothing fills them
//! until the player has already paid the stutter. This module runs the freshly
//! installed game once, in a controlled way, with every shader cache pointed at
//! a folder Ludex owns, and stops it as soon as the caches stop growing.
//!
//! # What is actually possible
//!
//! There is no portable, reliable "compile the shaders, do not show a window"
//! switch. DXVK/DXVK-NVAPI/VKD3D-Proton compile on demand, and the game decides
//! which pipelines it needs; Mesa compiles when the driver first sees a shader.
//! A true headless precompile would need per-engine knowledge (a benchmark
//! mode, a `-precompile` flag) that most of these games do not ship. This
//! module therefore implements the honest version, documented in
//! `docs/SHADERS.md` as a **warm start**:
//!
//! - the game is started exactly like a normal launch (same UMU/Proton/Wine
//!   plan, same Gamescope profile when the game uses one);
//! - every cache path is pinned to `{appDataDir}/shader-cache/{gameId}`, so the
//!   run fills caches Ludex can measure, clear, and hand back to the next
//!   launch;
//! - the run is bounded: a time budget (3 minutes by default) and a plateau
//!   rule (the caches stopped growing for 25 s) both stop it;
//! - the process tree is killed on cancel, on the plateau and at the budget;
//! - the job is a background task: it never blocks the first launch, and a
//!   failed preparation only marks the record, never the install.
//!
//! The normal launch reuses the same cache paths whenever the game has
//! preparation enabled, which is what turns the warm start into a real win.
//!
//! # Testability
//!
//! Nothing in [`supervise`] spawns a process: the runner is the [`PrepChild`]
//! trait, so timeout, cancel, plateau and failure paths are covered without
//! launching a game.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::settings::LocalSettings;
use crate::state::AppState;

pub(crate) mod queue;

pub use queue::{on_install_complete, recover_interrupted, ShaderPrepQueue};

/// Sub-folder of the app data folder that holds every prepared cache.
pub const CACHE_DIR: &str = "shader-cache";

/// Default time budget of one warm start. Long enough for a title's boot
/// sequence and first level on a cold prefix, short enough that a forgotten job
/// never holds the machine for an hour.
pub const DEFAULT_BUDGET_SECS: u64 = 180;

/// A smaller budget cannot outlast a Wine prefix creation on a slow disk.
pub const MIN_BUDGET_SECS: u64 = 30;

/// The upper bound is a stranger's patience, not a technical limit.
pub const MAX_BUDGET_SECS: u64 = 900;

/// How long the caches may stop growing before the run is considered warm.
pub const PLATEAU: Duration = Duration::from_secs(25);

/// How often the supervisor samples the caches and the process.
///
/// One second keeps the cancel latency imperceptible without hammering the
/// filesystem: a cache directory holds a few dozen files at most.
pub const POLL: Duration = Duration::from_secs(1);

/// Clamp a stored budget to the documented bounds.
pub fn clamp_budget(secs: u64) -> u64 {
    if secs == 0 {
        DEFAULT_BUDGET_SECS
    } else {
        secs.clamp(MIN_BUDGET_SECS, MAX_BUDGET_SECS)
    }
}

/* --------------------------------- state ---------------------------------- */

/// Where a game's preparation stands. Persisted in `settings.json` (see
/// [`LocalSettings::shader_prep`]); the desktop never adds a schema version for
/// this, because the state is a device preference like the overlay exclusions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PrepState {
    /// Never prepared: no cache folder, nothing to show.
    #[default]
    Never,
    Queued,
    Running,
    /// The warm start completed; `preparedAt` and `cacheBytes` are meaningful.
    Done,
    /// The warm start failed. The game is still launchable.
    Failed,
    /// The user cancelled it.
    Cancelled,
}

impl PrepState {
    pub fn as_str(self) -> &'static str {
        match self {
            PrepState::Never => "never",
            PrepState::Queued => "queued",
            PrepState::Running => "running",
            PrepState::Done => "done",
            PrepState::Failed => "failed",
            PrepState::Cancelled => "cancelled",
        }
    }

    /// True while the queue owns the game.
    pub fn is_active(self) -> bool {
        matches!(self, PrepState::Queued | PrepState::Running)
    }
}

/// Per-game preparation state, persisted under `shaderPrep` in the settings
/// document, keyed by the game id as a string (JSON object keys).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PrepRecord {
    pub state: PrepState,
    /// "Prepare after install" for this game. Falls back to the global
    /// `shaderPrepAfterInstall` when the record does not exist.
    pub enabled: bool,
    /// RFC 3339 time of the last successful preparation.
    pub prepared_at: Option<String>,
    /// Size of the cache folder at the last sample.
    pub cache_bytes: u64,
    /// How long the last run lasted.
    pub duration_ms: Option<u64>,
    /// Why the last run failed, was cancelled, or how it was stopped.
    pub message: Option<String>,
}

impl PrepRecord {
    /// Seed a record for a game that never ran preparation.
    pub fn seeded(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }
}

/// Read the record of one game. A missing record is `None`, never an error.
pub fn record(settings: &LocalSettings, game_id: i64) -> Option<&PrepRecord> {
    settings.shader_prep.get(&game_id.to_string())
}

/// True when this game's next install should be prepared automatically.
///
/// A per-game choice wins over the global default so a single title can opt
/// out (or in) without touching every other game.
pub fn enabled_for(settings: &LocalSettings, game_id: i64) -> bool {
    record(settings, game_id)
        .map(|entry| entry.enabled)
        .unwrap_or(settings.shader_prep_after_install)
}

/// Whether preparation can do anything on this platform. The caches this
/// module fills (DXVK, VKD3D-Proton, Mesa, the NVIDIA GL cache) are Linux
/// concepts; Windows and macOS report `false` and the UI hides the panel.
pub const fn supported() -> bool {
    cfg!(all(unix, not(target_os = "macos")))
}

/* ------------------------------- cache paths ------------------------------- */

/// Every cache path a warm start writes to, rooted in one folder so the size
/// is one recursive walk and "clear" is one `remove_dir_all`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLayout {
    pub root: PathBuf,
    /// `DXVK_STATE_CACHE_PATH`: pipeline state caches (`.dxvk-cache`).
    pub dxvk: PathBuf,
    /// `VKD3D_SHADER_CACHE_PATH`: D3D12 shader caches.
    pub vkd3d: PathBuf,
    /// `MESA_SHADER_CACHE_DIR`: RADV/ANV/GL driver shader caches.
    pub mesa: PathBuf,
    /// `__GL_SHADER_DISK_CACHE_PATH`: the proprietary NVIDIA GL cache.
    pub nvidia: PathBuf,
}

/// Resolve the cache folder of one game.
///
/// The folder lives in the app data directory, not in the prefix: a prefix
/// reset or a game uninstall must not silently delete a cache the user asked
/// to keep, and a prefix on a slow drive would make every launch read it.
pub fn cache_layout(app_data: &Path, game_id: i64) -> CacheLayout {
    let root = app_data.join(CACHE_DIR).join(game_id.to_string());
    CacheLayout {
        dxvk: root.join("dxvk"),
        vkd3d: root.join("vkd3d"),
        mesa: root.join("mesa"),
        nvidia: root.join("nvidia"),
        root,
    }
}

/// Environment a warm start (and every later launch) needs for the caches to
/// land in [`CacheLayout`].
///
/// Every variable is a documented upstream one:
/// `DXVK_STATE_CACHE_PATH`, `VKD3D_SHADER_CACHE_PATH`, `MESA_SHADER_CACHE_DIR`
/// and `__GL_SHADER_DISK_CACHE_PATH`. `DXVK_STATE_CACHE` is written explicitly
/// because a Proton prefix may carry `DXVK_STATE_CACHE=0` from an older tweak.
pub fn cache_env(layout: &CacheLayout) -> Vec<(&'static str, String)> {
    vec![
        ("DXVK_STATE_CACHE", "1".to_string()),
        (
            "DXVK_STATE_CACHE_PATH",
            layout.dxvk.to_string_lossy().to_string(),
        ),
        (
            "VKD3D_SHADER_CACHE_PATH",
            layout.vkd3d.to_string_lossy().to_string(),
        ),
        (
            "MESA_SHADER_CACHE_DIR",
            layout.mesa.to_string_lossy().to_string(),
        ),
        ("MESA_SHADER_CACHE_DISABLE", "false".to_string()),
        ("__GL_SHADER_DISK_CACHE", "1".to_string()),
        (
            "__GL_SHADER_DISK_CACHE_PATH",
            layout.nvidia.to_string_lossy().to_string(),
        ),
        ("__GL_SHADER_DISK_CACHE_SKIP_CLEANUP", "1".to_string()),
    ]
}

/// Cache environment for a normal launch, minus whatever the user already set
/// in the game's own environment: an explicit value is a deliberate choice and
/// is never overridden.
pub fn launch_env(
    layout: &CacheLayout,
    user_env: &BTreeMap<String, String>,
) -> Vec<(&'static str, String)> {
    cache_env(layout)
        .into_iter()
        .filter(|(key, _)| !user_env.contains_key(*key))
        .collect()
}

/// Create the four cache folders. A folder already there is not an error.
pub fn ensure_cache_dirs(layout: &CacheLayout) -> Result<(), String> {
    for dir in [&layout.dxvk, &layout.vkd3d, &layout.mesa, &layout.nvidia] {
        std::fs::create_dir_all(dir).map_err(|error| {
            format!(
                "Could not create the shader cache folder {}: {error}",
                dir.display()
            )
        })?;
    }
    Ok(())
}

/// Total size of a folder tree, following the rule "files only": symlinks are
/// skipped (a game may link a cache elsewhere; counting the target twice would
/// make the number lie) and an unreadable entry is ignored.
pub fn dir_size(root: &Path) -> u64 {
    fn walk(dir: &Path, total: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                walk(&entry.path(), total);
            } else if meta.is_file() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
    let mut total = 0;
    walk(root, &mut total);
    total
}

/// Delete the cache folder. Returns whether anything was there.
pub fn clear_cache(layout: &CacheLayout) -> Result<bool, String> {
    match std::fs::remove_dir_all(&layout.root) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Could not clear the shader cache {}: {error}",
            layout.root.display()
        )),
    }
}

/* ------------------------------- supervisor -------------------------------- */

/// How one warm start ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The game exited on its own.
    Exited,
    /// The caches stopped growing for [`PLATEAU`].
    Plateau,
    /// The time budget ran out.
    Budget,
}

impl StopReason {
    /// Sentence stored on a successful record, `None` when the game exited by
    /// itself (the common, unambiguous case).
    pub fn message(self) -> Option<&'static str> {
        match self {
            StopReason::Exited => None,
            StopReason::Plateau => Some("Stopped after the shader cache stopped growing."),
            StopReason::Budget => Some("Stopped when the time budget ran out."),
        }
    }
}

/// Result of a supervised run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done { bytes: u64, reason: StopReason },
    Cancelled { bytes: u64 },
    Failed { message: String, bytes: u64 },
}

/// One child process under supervision.
///
/// The real implementation wraps `std::process::Child`; tests implement it to
/// simulate exits, silence and cache growth without spawning a game.
pub trait PrepChild: Send {
    /// `Ok(None)` while the process runs, `Ok(Some(code))` once it exited.
    fn poll(&mut self) -> Result<Option<Option<i32>>, String>;
    /// Kill the process tree. Idempotent; called at most once by [`supervise`].
    fn stop(&mut self);
}

/// One progress sample, forwarded to the UI by the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrepTick {
    pub bytes: u64,
    pub elapsed: Duration,
}

/// Budget and sampling of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Supervision {
    pub budget: Duration,
    pub poll: Duration,
    pub plateau: Duration,
}

impl Supervision {
    /// Production timing: the configured budget, one-second samples and the
    /// documented plateau.
    pub fn new(budget: Duration) -> Self {
        Self {
            budget,
            poll: POLL,
            plateau: PLATEAU,
        }
    }

    /// Faster sampling for tests; the caller still injects the child.
    #[cfg(test)]
    fn quick(budget: Duration) -> Self {
        Self {
            budget,
            poll: Duration::from_millis(2),
            plateau: Duration::from_millis(20),
        }
    }
}

/// Supervise one warm start until it exits, plateaus, times out or is
/// cancelled.
///
/// `cancelled` and `report` are closures because the queue needs to read a
/// flag and emit an event, while tests pass a plain flag and a no-op. The
/// function never panics on a broken child: every failure becomes
/// [`Outcome::Failed`].
pub fn supervise(
    child: &mut dyn PrepChild,
    cache_root: &Path,
    cfg: &Supervision,
    mut cancelled: impl FnMut() -> bool,
    mut report: impl FnMut(PrepTick),
) -> Outcome {
    let started = Instant::now();
    let mut bytes = dir_size(cache_root);
    let mut last_growth = started;

    loop {
        if cancelled() {
            child.stop();
            let bytes = dir_size(cache_root);
            return Outcome::Cancelled { bytes };
        }
        match child.poll() {
            Ok(Some(code)) => {
                let bytes = dir_size(cache_root);
                report(PrepTick {
                    bytes,
                    elapsed: started.elapsed(),
                });
                if code == Some(0) || code.is_none() {
                    // A clean exit that cached nothing warmed nothing: report
                    // it instead of claiming "prepared" over an empty folder.
                    if bytes == 0 {
                        return Outcome::Failed {
                            message:
                                "The game exited without writing a shader cache. \
                                      Run preparation again while the game can reach its main menu."
                                    .to_string(),
                            bytes,
                        };
                    }
                    return Outcome::Done {
                        bytes,
                        reason: StopReason::Exited,
                    };
                }
                return Outcome::Failed {
                    message: describe_exit(code, bytes),
                    bytes,
                };
            }
            Ok(None) => {}
            Err(error) => {
                child.stop();
                return Outcome::Failed {
                    message: format!("Could not watch the game process: {error}"),
                    bytes: dir_size(cache_root),
                };
            }
        }

        let now = Instant::now();
        let sample = dir_size(cache_root);
        if sample > bytes {
            bytes = sample;
            last_growth = now;
        }
        report(PrepTick {
            bytes,
            elapsed: now.duration_since(started),
        });

        if now.duration_since(started) >= cfg.budget {
            child.stop();
            let bytes = dir_size(cache_root);
            return if bytes == 0 {
                Outcome::Failed {
                    message: "The time budget ran out before the game wrote a shader cache. \
                              The game probably needs more time on this prefix; run preparation again."
                        .to_string(),
                    bytes,
                }
            } else {
                Outcome::Done {
                    bytes,
                    reason: StopReason::Budget,
                }
            };
        }
        if bytes > 0 && now.duration_since(last_growth) >= cfg.plateau {
            child.stop();
            return Outcome::Done {
                bytes: dir_size(cache_root),
                reason: StopReason::Plateau,
            };
        }
        std::thread::sleep(cfg.poll);
    }
}

/// Failure message naming the exit status and what was cached regardless.
fn describe_exit(code: Option<i32>, bytes: u64) -> String {
    let status = match code {
        Some(code) => format!("exit code {code}"),
        None => "a signal".to_string(),
    };
    if bytes == 0 {
        format!("The game stopped with {status} before writing a shader cache.")
    } else {
        format!(
            "The game stopped with {status} after writing {} of shader cache. \
             The cache is kept and the game launches normally.",
            crate::util::human_bytes(bytes)
        )
    }
}

/// Turn an outcome into the persisted record, keeping the per-game toggle.
pub fn record_for_outcome(
    previous: &PrepRecord,
    outcome: &Outcome,
    now: &str,
    duration_ms: u64,
) -> PrepRecord {
    let mut next = previous.clone();
    next.duration_ms = Some(duration_ms);
    match outcome {
        Outcome::Done { bytes, reason } => {
            next.state = PrepState::Done;
            next.cache_bytes = *bytes;
            next.prepared_at = Some(now.to_string());
            next.message = reason.message().map(str::to_string);
        }
        Outcome::Cancelled { bytes } => {
            next.state = PrepState::Cancelled;
            next.cache_bytes = *bytes;
            next.message = Some("Cancelled. The partial cache is kept.".to_string());
        }
        Outcome::Failed { message, bytes } => {
            next.state = PrepState::Failed;
            next.cache_bytes = *bytes;
            next.message = Some(message.clone());
        }
    }
    next
}

/* --------------------------------- spawning -------------------------------- */

/// Everything needed to start a warm start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    pub env: BTreeMap<String, String>,
}

/// Starts the game for a warm start.
pub trait PrepSpawner: Send {
    fn spawn(&mut self, spec: &PrepCommand) -> Result<Box<dyn PrepChild>, String>;
}

/// Real spawner: `std::process::Command` with the resolved compat plan.
#[derive(Debug, Default, Clone, Copy)]
pub struct CommandSpawner;

impl PrepSpawner for CommandSpawner {
    fn spawn(&mut self, spec: &PrepCommand) -> Result<Box<dyn PrepChild>, String> {
        let mut command = std::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.working_dir)
            .envs(spec.env.iter())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = command.spawn().map_err(|error| {
            format!(
                "Could not start {} for shader preparation: {error}",
                spec.program.display()
            )
        })?;
        Ok(Box::new(CommandChild {
            pid: child.id(),
            child,
        }))
    }
}

struct CommandChild {
    pid: u32,
    child: std::process::Child,
}

impl PrepChild for CommandChild {
    fn poll(&mut self) -> Result<Option<Option<i32>>, String> {
        self.child
            .try_wait()
            .map(|status| status.map(|status| status.code()))
            .map_err(|error| error.to_string())
    }

    fn stop(&mut self) {
        // The compat wrapper spawns the real game, so killing the root alone
        // leaves wineserver and the game running.
        crate::process_tree::ProcessSampler::new().kill_tree(self.pid);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn the injected child and supervise it. The seam tests use: production
/// passes [`CommandSpawner`], a test passes a fake that never launches a game.
pub fn run_with(
    spawner: &mut dyn PrepSpawner,
    spec: &PrepCommand,
    layout: &CacheLayout,
    cfg: &Supervision,
    cancelled: impl FnMut() -> bool,
    report: impl FnMut(PrepTick),
) -> Outcome {
    if let Err(message) = ensure_cache_dirs(layout) {
        return Outcome::Failed { message, bytes: 0 };
    }
    match spawner.spawn(spec) {
        Ok(mut child) => supervise(child.as_mut(), &layout.root, cfg, cancelled, report),
        Err(message) => Outcome::Failed { message, bytes: 0 },
    }
}

/// Point a normal launch at the prepared caches (#293).
///
/// Only when the game opted into preparation: a game that never prepared must
/// keep the historical environment byte for byte. A per-game variable wins, so
/// a user who pinned `DXVK_STATE_CACHE_PATH` is never overridden.
pub fn apply_launch_env(
    app: &AppHandle,
    game_id: i64,
    options: &crate::linux_gaming::LaunchOptions,
    command: &mut std::process::Command,
) {
    if !supported() {
        return;
    }
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    // Reuse when the game opted into preparation, or when a manual "Prepare
    // now" actually left a cache: a manual run must not be a no-op.
    let prepared = record(&settings, game_id).is_some_and(|entry| entry.state == PrepState::Done);
    if !prepared && !enabled_for(&settings, game_id) {
        return;
    }
    let layout = cache_layout(&state.app_data, game_id);
    for (key, value) in launch_env(&layout, &options.env) {
        command.env(key, value);
    }
}

/* ------------------------------- the queue -------------------------------- */

/// One queued or running game, kept in memory: a restart loses the queue but
/// never a cache, and [`recover_interrupted`] turns the stale records honest.
#[derive(Debug, Clone)]
struct Pending {
    game_id: i64,
    title: String,
}

#[derive(Debug)]
struct Running {
    game_id: i64,
    cancel: Arc<AtomicBool>,
}

/// Serialises warm starts: one game at a time, newest request appended.
///
/// The queue is deliberately separate from the install scheduler: an install
/// finishing is the trigger, but a prep job never takes a download/extract slot
/// and never delays a queued install.
#[derive(Debug, Default)]
pub struct QueueState {
    pending: Vec<Pending>,
    running: Option<Running>,
    /// True while a worker thread is alive. Guarded by the same mutex as the
    /// queue so two enqueues can never start two workers.
    worker: bool,
}

/// Shared queue handle stored in [`AppState`].
pub struct Queue {
    inner: Mutex<QueueState>,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(QueueState::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// True when this game is queued or running.
    pub fn is_active(&self, game_id: i64) -> bool {
        let state = self.lock();
        state.pending.iter().any(|entry| entry.game_id == game_id)
            || state
                .running
                .as_ref()
                .is_some_and(|entry| entry.game_id == game_id)
    }

    /// Snapshot for the UI: `(game_id, running)` of everything the queue owns.
    pub fn active_games(&self) -> Vec<i64> {
        let state = self.lock();
        let mut games: Vec<i64> = state.pending.iter().map(|entry| entry.game_id).collect();
        if let Some(running) = state.running.as_ref() {
            games.push(running.game_id);
        }
        games
    }

    /// Queue a game. Returns `false` when it was already queued.
    fn push(&self, entry: Pending) -> bool {
        let mut state = self.lock();
        if state
            .running
            .as_ref()
            .is_some_and(|running| running.game_id == entry.game_id)
            || state
                .pending
                .iter()
                .any(|pending| pending.game_id == entry.game_id)
        {
            return false;
        }
        state.pending.push(entry);
        true
    }

    /// Pop the next game and mark it running. `None` when the queue is empty;
    /// the caller then clears [`QueueState::worker`].
    fn next(&self) -> Option<(Pending, Arc<AtomicBool>)> {
        let mut state = self.lock();
        let entry = state.pending.first().cloned()?;
        state.pending.remove(0);
        let cancel = Arc::new(AtomicBool::new(false));
        state.running = Some(Running {
            game_id: entry.game_id,
            cancel: Arc::clone(&cancel),
        });
        Some((entry, cancel))
    }

    fn finish(&self) {
        self.lock().running = None;
    }

    /// Ask the running game to stop, or drop it from the queue. Returns
    /// `(was_running, was_queued)`.
    fn cancel(&self, game_id: i64) -> (bool, bool) {
        let mut state = self.lock();
        let running = state
            .running
            .as_ref()
            .is_some_and(|entry| entry.game_id == game_id);
        if running {
            if let Some(entry) = state.running.as_ref() {
                entry.cancel.store(true, Ordering::SeqCst);
            }
        }
        let had_pending = state.pending.iter().any(|entry| entry.game_id == game_id);
        state.pending.retain(|entry| entry.game_id != game_id);
        (running, had_pending)
    }

    /// Claim the single worker slot. `false` when a worker already runs.
    fn claim_worker(&self) -> bool {
        let mut state = self.lock();
        if state.worker {
            return false;
        }
        state.worker = true;
        true
    }

    fn release_worker(&self) {
        self.lock().worker = false;
    }

    /// Give the worker slot back, unless an enqueue raced the empty check.
    /// `true` means the worker may stop.
    fn retire_worker(&self) -> bool {
        let mut state = self.lock();
        if state.pending.is_empty() {
            state.worker = false;
            true
        } else {
            false
        }
    }
}

/* ------------------------------- commands --------------------------------- */

/// The serialisable per-game view shown on the game page and in Downloads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderPrepView {
    pub game_id: i64,
    pub state: PrepState,
    pub enabled: bool,
    pub prepared_at: Option<String>,
    pub cache_bytes: u64,
    pub duration_ms: Option<u64>,
    pub message: Option<String>,
    pub cache_path: String,
    pub budget_secs: u64,
    pub supported: bool,
}

/// Progress event payload (`shader-premp://progress`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShaderPrepProgress {
    pub game_id: i64,
    pub title: String,
    pub state: PrepState,
    pub cache_bytes: u64,
    pub elapsed_ms: u64,
    pub budget_secs: u64,
    pub message: Option<String>,
}

/// State read straight from the settings document.
pub fn view_for(
    settings: &LocalSettings,
    app_data: &Path,
    game_id: i64,
    budget_secs: u64,
) -> ShaderPrepView {
    let layout = cache_layout(app_data, game_id);
    let entry = record(settings, game_id).cloned().unwrap_or_default();
    ShaderPrepView {
        game_id,
        state: entry.state,
        enabled: enabled_for(settings, game_id),
        prepared_at: entry.prepared_at,
        cache_bytes: dir_size(&layout.root),
        duration_ms: entry.duration_ms,
        message: entry.message,
        cache_path: layout.root.to_string_lossy().to_string(),
        budget_secs,
        supported: supported(),
    }
}

/* --------------------------------- tests ---------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ludex-shaders-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("tempdir");
        dir
    }

    #[test]
    fn cache_paths_are_rooted_in_app_data_and_never_in_the_prefix() {
        let app_data = PathBuf::from("/data/ludex");
        let layout = cache_layout(&app_data, 42);
        assert_eq!(layout.root, Path::new("/data/ludex/shader-cache/42"));
        assert_eq!(layout.dxvk, Path::new("/data/ludex/shader-cache/42/dxvk"));
        assert_eq!(layout.vkd3d, Path::new("/data/ludex/shader-cache/42/vkd3d"));
        assert_eq!(layout.mesa, Path::new("/data/ludex/shader-cache/42/mesa"));
        assert_eq!(
            layout.nvidia,
            Path::new("/data/ludex/shader-cache/42/nvidia")
        );
        // Two games never share a folder.
        assert_ne!(
            cache_layout(&app_data, 42).root,
            cache_layout(&app_data, 43).root
        );
    }

    #[test]
    fn the_catalog_names_every_cache_the_drivers_read() {
        let layout = cache_layout(Path::new("/data"), 7);
        let env: BTreeMap<&str, String> =
            cache_env(&layout).into_iter().collect::<BTreeMap<_, _>>();
        for key in [
            "DXVK_STATE_CACHE",
            "DXVK_STATE_CACHE_PATH",
            "VKD3D_SHADER_CACHE_PATH",
            "MESA_SHADER_CACHE_DIR",
            "MESA_SHADER_CACHE_DISABLE",
            "__GL_SHADER_DISK_CACHE",
            "__GL_SHADER_DISK_CACHE_PATH",
            "__GL_SHADER_DISK_CACHE_SKIP_CLEANUP",
        ] {
            assert!(env.contains_key(key), "{key} missing");
        }
        assert_eq!(env["DXVK_STATE_CACHE"], "1");
        assert_eq!(env["DXVK_STATE_CACHE_PATH"], "/data/shader-cache/7/dxvk");
        assert_eq!(env["MESA_SHADER_CACHE_DIR"], "/data/shader-cache/7/mesa");
    }

    #[test]
    fn a_user_environment_value_is_never_overridden() {
        let layout = cache_layout(Path::new("/data"), 3);
        let mut user = BTreeMap::new();
        user.insert(
            "DXVK_STATE_CACHE_PATH".to_string(),
            "/games/custom-dxvk".to_string(),
        );
        let env = launch_env(&layout, &user);
        assert!(env.iter().all(|(key, _)| *key != "DXVK_STATE_CACHE_PATH"));
        assert!(env.iter().any(|(key, _)| *key == "MESA_SHADER_CACHE_DIR"));
    }

    #[test]
    fn size_accounting_walks_nested_files_and_ignores_symlinks() {
        let dir = temp_dir("size");
        std::fs::create_dir_all(dir.join("dxvk/sub")).expect("dirs");
        std::fs::write(dir.join("dxvk/a.cache"), vec![0u8; 100]).expect("write");
        std::fs::write(dir.join("dxvk/sub/b.cache"), vec![0u8; 250]).expect("write");
        std::fs::write(dir.join("empty"), b"").expect("write");

        assert_eq!(dir_size(&dir), 350);

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("dxvk/a.cache"), dir.join("link"))
                .expect("symlink");
            assert_eq!(dir_size(&dir), 350, "a symlink must not be counted");
        }

        // A missing folder is empty, not an error.
        assert_eq!(dir_size(&dir.join("missing")), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clearing_removes_the_tree_and_reports_whether_it_existed() {
        let app_data = temp_dir("clear");
        let layout = cache_layout(&app_data, 11);
        ensure_cache_dirs(&layout).expect("dirs");
        std::fs::write(layout.dxvk.join("state.dxvk-cache"), vec![0u8; 64]).expect("write");
        assert_eq!(dir_size(&layout.root), 64);

        assert_eq!(clear_cache(&layout), Ok(true));
        assert!(!layout.root.exists());
        assert_eq!(dir_size(&layout.root), 0);
        // Clearing twice is not an error: the panel may race a manual delete.
        assert_eq!(clear_cache(&layout), Ok(false));
        std::fs::remove_dir_all(&app_data).ok();
    }

    #[test]
    fn a_successful_exit_is_done_with_whatever_it_cached() {
        let dir = temp_dir("exit");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::write(cache.join("state.dxvk-cache"), vec![0u8; 512]).expect("write");
        let mut child = FakeChild::exiting(Some(0));
        let outcome = supervise(
            &mut child,
            &cache,
            &Supervision::quick(Duration::from_secs(30)),
            || false,
            |_| {},
        );
        assert_eq!(
            outcome,
            Outcome::Done {
                bytes: 512,
                reason: StopReason::Exited
            }
        );
        assert!(!child.stopped, "a game that exited is not killed again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_zero_exit_is_a_failure_that_keeps_the_cache() {
        let dir = temp_dir("fail");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::write(cache.join("state.dxvk-cache"), vec![0u8; 128]).expect("write");
        let mut child = FakeChild::exiting(Some(3));
        let outcome = supervise(
            &mut child,
            &cache,
            &Supervision::quick(Duration::from_secs(30)),
            || false,
            |_| {},
        );
        match outcome {
            Outcome::Failed { message, bytes } => {
                assert_eq!(bytes, 128);
                assert!(message.contains("exit code 3"), "{message}");
                assert!(message.contains("launches normally"), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_time_budget_stops_a_game_that_never_exits() {
        let dir = temp_dir("budget");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::write(cache.join("state.dxvk-cache"), vec![0u8; 256]).expect("write");
        let mut child = FakeChild::running();
        let cfg = Supervision {
            budget: Duration::from_millis(40),
            poll: Duration::from_millis(2),
            // Longer than the budget: this test is about the time limit, not
            // about the cache plateau.
            plateau: Duration::from_secs(5),
        };
        let outcome = supervise(&mut child, &cache, &cfg, || false, |_| {});
        assert_eq!(
            outcome,
            Outcome::Done {
                bytes: 256,
                reason: StopReason::Budget
            }
        );
        assert!(child.stopped, "the tree must be killed at the budget");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_budget_without_any_cache_growth_is_a_failure() {
        let dir = temp_dir("budget-empty");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        let mut child = FakeChild::running();
        let outcome = supervise(
            &mut child,
            &cache,
            &Supervision::quick(Duration::from_millis(25)),
            || false,
            |_| {},
        );
        match outcome {
            Outcome::Failed { message, bytes } => {
                assert_eq!(bytes, 0);
                assert!(message.contains("time budget"), "{message}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(child.stopped);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancelling_stops_the_tree_and_keeps_the_partial_cache() {
        let dir = temp_dir("cancel");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::write(cache.join("state.dxvk-cache"), vec![0u8; 64]).expect("write");
        let mut child = FakeChild::running();
        let cancelled = AtomicBool::new(true);
        let outcome = supervise(
            &mut child,
            &cache,
            &Supervision::quick(Duration::from_secs(30)),
            || cancelled.load(Ordering::SeqCst),
            |_| {},
        );
        assert_eq!(outcome, Outcome::Cancelled { bytes: 64 });
        assert!(child.stopped);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn growth_that_stops_for_the_plateau_ends_the_run_early() {
        let dir = temp_dir("plateau");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        let mut child = FakeChild::writing_once(cache.join("state.dxvk-cache"));
        let cfg = Supervision {
            budget: Duration::from_secs(30),
            poll: Duration::from_millis(2),
            plateau: Duration::from_millis(20),
        };
        let outcome = supervise(&mut child, &cache, &cfg, || false, |_| {});
        assert_eq!(
            outcome,
            Outcome::Done {
                bytes: 900,
                reason: StopReason::Plateau
            }
        );
        assert!(child.stopped);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn progress_is_reported_at_least_once_and_counts_the_cache() {
        let dir = temp_dir("progress");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        std::fs::write(cache.join("state.dxvk-cache"), vec![0u8; 8]).expect("write");
        let mut child = FakeChild::exiting(Some(0));
        let samples = AtomicUsize::new(0);
        let seen = Mutex::new(Vec::new());
        let outcome = supervise(
            &mut child,
            &cache,
            &Supervision::quick(Duration::from_secs(30)),
            || false,
            |tick| {
                samples.fetch_add(1, Ordering::SeqCst);
                seen.lock().expect("samples").push(tick.bytes);
            },
        );
        assert!(matches!(outcome, Outcome::Done { .. }));
        assert!(samples.load(Ordering::SeqCst) >= 1);
        let seen = seen.lock().expect("samples");
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|bytes| *bytes == 8));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_clean_exit_with_an_empty_cache_is_not_a_success() {
        let dir = temp_dir("empty-exit");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).expect("cache");
        let mut child = FakeChild::exiting(Some(0));
        let outcome = supervise(
            &mut child,
            &cache,
            &Supervision::quick(Duration::from_secs(30)),
            || false,
            |_| {},
        );
        match outcome {
            Outcome::Failed { message, bytes } => {
                assert_eq!(bytes, 0);
                assert!(
                    message.contains("without writing a shader cache"),
                    "{message}"
                );
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_record_keeps_the_per_game_toggle_and_stamps_the_date() {
        let previous = PrepRecord {
            enabled: true,
            ..PrepRecord::default()
        };
        let done = record_for_outcome(
            &previous,
            &Outcome::Done {
                bytes: 1024,
                reason: StopReason::Plateau,
            },
            "2026-11-19T10:00:00Z",
            45_000,
        );
        assert_eq!(done.state, PrepState::Done);
        assert!(done.enabled, "the toggle must survive a run");
        assert_eq!(done.cache_bytes, 1024);
        assert_eq!(done.prepared_at.as_deref(), Some("2026-11-19T10:00:00Z"));
        assert_eq!(done.duration_ms, Some(45_000));
        assert!(done.message.is_some());

        let failed = record_for_outcome(
            &previous,
            &Outcome::Failed {
                message: "boom".to_string(),
                bytes: 12,
            },
            "2026-11-19T10:00:00Z",
            1_000,
        );
        assert_eq!(failed.state, PrepState::Failed);
        assert_eq!(failed.cache_bytes, 12);
        assert_eq!(failed.message.as_deref(), Some("boom"));
        assert_eq!(failed.prepared_at, None, "a failure is not a prepared date");
        assert!(failed.enabled);
    }

    #[test]
    fn the_budget_is_bounded_and_zero_falls_back_to_the_default() {
        assert_eq!(clamp_budget(0), DEFAULT_BUDGET_SECS);
        assert_eq!(clamp_budget(1), MIN_BUDGET_SECS);
        assert_eq!(clamp_budget(120), 120);
        assert_eq!(clamp_budget(u64::MAX), MAX_BUDGET_SECS);
    }

    #[test]
    fn the_install_default_only_applies_without_a_per_game_choice() {
        let mut settings = LocalSettings {
            shader_prep_after_install: true,
            ..LocalSettings::default()
        };
        assert!(enabled_for(&settings, 1));
        settings
            .shader_prep
            .insert("1".to_string(), PrepRecord::seeded(false));
        assert!(!enabled_for(&settings, 1), "the per-game opt-out wins");
        settings.shader_prep_after_install = false;
        settings
            .shader_prep
            .insert("2".to_string(), PrepRecord::seeded(true));
        assert!(enabled_for(&settings, 2), "the per-game opt-in wins");
        assert!(!enabled_for(&settings, 3));
    }

    #[test]
    fn the_queue_deduplicates_and_tracks_the_running_game() {
        let queue = Queue::new();
        assert!(queue.push(Pending {
            game_id: 5,
            title: "Hades".into()
        }));
        assert!(!queue.push(Pending {
            game_id: 5,
            title: "Hades".into()
        }));
        assert_eq!(queue.active_games(), vec![5]);
        assert!(queue.is_active(5));
        let (entry, cancel) = queue.next().expect("running");
        assert_eq!(entry.game_id, 5);
        assert!(!queue.push(Pending {
            game_id: 5,
            title: "Hades".into()
        }));
        assert!(!cancel.load(Ordering::SeqCst));
        assert_eq!(queue.cancel(5), (true, false));
        assert!(cancel.load(Ordering::SeqCst));
        queue.finish();
        assert!(!queue.is_active(5));
    }

    #[test]
    fn cancelling_a_queued_game_drops_it_without_touching_the_runner() {
        let queue = Queue::new();
        queue.push(Pending {
            game_id: 1,
            title: "A".into(),
        });
        queue.push(Pending {
            game_id: 2,
            title: "B".into(),
        });
        let (first, _) = queue.next().expect("running");
        assert_eq!(first.game_id, 1);
        assert_eq!(queue.cancel(2), (false, true));
        assert_eq!(queue.active_games(), vec![1]);
        queue.finish();
        assert!(queue.next().is_none());
    }

    #[test]
    fn a_failed_spawn_is_a_failure_not_a_panic() {
        let dir = temp_dir("spawn-fail");
        let layout = cache_layout(&dir, 9);
        let mut spawner = FakeSpawner {
            fail: Some("no compat tool".to_string()),
            child: None,
            seen: Vec::new(),
        };
        let outcome = run_with(
            &mut spawner,
            &PrepCommand {
                program: PathBuf::from("/usr/bin/umu-run"),
                args: vec!["/games/Hades.exe".to_string()],
                working_dir: dir.clone(),
                env: BTreeMap::new(),
            },
            &layout,
            &Supervision::quick(Duration::from_secs(30)),
            || false,
            |_| {},
        );
        assert_eq!(
            outcome,
            Outcome::Failed {
                message: "no compat tool".to_string(),
                bytes: 0
            }
        );
        // The folders exist anyway: the next attempt only has to spawn.
        assert!(layout.dxvk.is_dir());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_injected_runner_receives_the_cache_environment() {
        let dir = temp_dir("runner");
        let layout = cache_layout(&dir, 4);
        let cache = layout.root.clone();
        let mut spec_env = BTreeMap::new();
        for (key, value) in cache_env(&layout) {
            spec_env.insert(key.to_string(), value);
        }
        let mut spawner = FakeSpawner {
            fail: None,
            child: Some(FakeChildAt {
                cache: cache.clone(),
                exit_after: 1,
                polls: 0,
                stopped: false,
            }),
            seen: Vec::new(),
        };
        let outcome = run_with(
            &mut spawner,
            &PrepCommand {
                program: PathBuf::from("/usr/bin/umu-run"),
                args: vec!["/games/Hades.exe".to_string()],
                working_dir: dir.clone(),
                env: spec_env,
            },
            &layout,
            &Supervision::quick(Duration::from_secs(30)),
            || false,
            |_| {},
        );
        assert_eq!(spawner.seen.len(), 1, "exactly one launch");
        let seen = &spawner.seen[0];
        assert_eq!(
            seen.env.get("DXVK_STATE_CACHE_PATH").map(String::as_str),
            Some(layout.dxvk.to_string_lossy().as_ref())
        );
        assert!(matches!(
            outcome,
            Outcome::Failed { .. } | Outcome::Done { .. }
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Spawner that never launches a game: tests inject the child.
    struct FakeSpawner {
        fail: Option<String>,
        child: Option<FakeChildAt>,
        seen: Vec<PrepCommand>,
    }

    impl PrepSpawner for FakeSpawner {
        fn spawn(&mut self, spec: &PrepCommand) -> Result<Box<dyn PrepChild>, String> {
            self.seen.push(spec.clone());
            match self.child.take() {
                Some(child) => Ok(Box::new(child)),
                None => Err(self.fail.clone().unwrap_or_else(|| "no child".to_string())),
            }
        }
    }

    /// Child that writes one cache file then exits non-zero, proving the
    /// failure path keeps what was cached.
    struct FakeChildAt {
        cache: PathBuf,
        exit_after: usize,
        polls: usize,
        stopped: bool,
    }

    impl PrepChild for FakeChildAt {
        fn poll(&mut self) -> Result<Option<Option<i32>>, String> {
            self.polls += 1;
            if self.polls == 1 {
                std::fs::write(self.cache.join("dxvk/state.dxvk-cache"), vec![0u8; 40])
                    .expect("cache write");
                return Ok(None);
            }
            if self.polls > self.exit_after {
                return Ok(Some(Some(1)));
            }
            Ok(None)
        }

        fn stop(&mut self) {
            self.stopped = true;
        }
    }

    /// Child used by every supervision test.
    struct FakeChild {
        exit: Option<Option<i32>>,
        write: Option<PathBuf>,
        wrote: bool,
        polls: usize,
        stopped: bool,
    }

    impl FakeChild {
        fn exiting(code: Option<i32>) -> Self {
            Self {
                exit: Some(code),
                write: None,
                wrote: false,
                polls: 0,
                stopped: false,
            }
        }

        fn running() -> Self {
            Self {
                exit: None,
                write: None,
                wrote: false,
                polls: 0,
                stopped: false,
            }
        }

        fn writing_once(path: PathBuf) -> Self {
            Self {
                exit: None,
                write: Some(path),
                wrote: false,
                polls: 0,
                stopped: false,
            }
        }
    }

    impl PrepChild for FakeChild {
        fn poll(&mut self) -> Result<Option<Option<i32>>, String> {
            self.polls += 1;
            if let Some(path) = self.write.clone() {
                if !self.wrote {
                    self.wrote = true;
                    std::fs::write(path, vec![0u8; 900]).expect("write");
                }
            }
            if self.polls > 1 {
                return Ok(self.exit);
            }
            Ok(None)
        }

        fn stop(&mut self) {
            self.stopped = true;
        }
    }
}
