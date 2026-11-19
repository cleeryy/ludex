//! Keep the machine awake while Ludex is working (#288).
//!
//! A console downloads while nobody is looking. Without an inhibitor the screen
//! blanks, the machine suspends, and the transfer, the extraction and the
//! streaming host all die silently. This module holds one
//! `systemd-inhibit --what=idle:sleep:handle-lid-switch --mode=block ... sleep
//! infinity` child for the process lifetime while at least one reason is
//! active:
//!
//! - an install job is transferring, verifying or extracting;
//! - an install is being checked from the game page;
//! - a game (or a ROM emulator) is running;
//! - a Moonlight stream was started by Ludex.
//!
//! Reasons are a keyed set, not a counter: a job registers one key and releases
//! it once, so a duplicated acquire or release can never leak the lock. The
//! last release starts the grace window ("keep the machine awake for N minutes
//! after the last job"), then the child is killed and reaped — `Drop` and the
//! exit hook both guarantee that `systemd-inhibit --list` never shows a stale
//! Ludex entry.
//!
//! On a platform without `systemd-inhibit` the whole feature is a silent no-op:
//! the reasons are still tracked (so the status view is honest about what is
//! running) but `supported` is false and no error reaches the user.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

/// What the inhibitor blocks: idle detection, sleep, and the lid switch.
///
/// `handle-lid-switch` is what keeps a laptop downloading with the lid closed;
/// it is deliberately not `shutdown` (a user-requested power-off still works).
pub const WHAT: &str = "idle:sleep:handle-lid-switch";

/// The `Who=` field shown by `systemd-inhibit --list`.
pub const WHO: &str = "Ludex";

/// Default: stay awake five minutes after the last job.
pub const DEFAULT_GRACE_MINUTES: u32 = 5;

/// `0` releases the inhibitor the moment the last job ends.
pub const MIN_GRACE_MINUTES: u32 = 0;

/// A longer window would silently disable the user's power settings for an hour.
pub const MAX_GRACE_MINUTES: u32 = 60;

/// The Linux command this module drives. Resolved through `PATH` at startup.
/// Test builds replace the spawner with [`UnavailableSpawner`], so this name
/// is only read by a release build.
#[cfg_attr(test, allow(dead_code))]
const COMMAND: &str = "systemd-inhibit";

/// How long the wrapper gets to fail loudly before it is assumed healthy.
///
/// `systemd-inhibit` reports a missing logind by exiting a few milliseconds
/// after start; without this probe Ludex would believe it holds a lock it does
/// not have.
const READY_PROBE: Duration = Duration::from_millis(150);

/// Clamp the persisted setting to the documented bounds.
pub fn clamp_grace_minutes(minutes: u32) -> u32 {
    minutes.clamp(MIN_GRACE_MINUTES, MAX_GRACE_MINUTES)
}

fn grace_duration(minutes: u32) -> Duration {
    Duration::from_secs(u64::from(clamp_grace_minutes(minutes)) * 60)
}

/* --------------------------------- reasons -------------------------------- */

/// Why the machine must stay awake. Exposed to the web shell so the banner can
/// name the work instead of saying "something".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasonKind {
    /// An install job is downloading, verifying or extracting.
    Install,
    /// The game page is checking an installed folder against its manifest.
    Verify,
    /// A game or emulator process is running.
    Game,
    /// A Moonlight stream started by Ludex is running.
    Stream,
}

/// One holder of the inhibitor. The key is the identity: two installs are two
/// reasons, and a duplicated `add` for the same key is a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub key: String,
    pub kind: ReasonKind,
    pub label: String,
}

fn clean_label(label: &str, fallback: &str) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

impl Reason {
    pub fn install(job_id: &str, title: &str) -> Self {
        Self {
            key: format!("install:{job_id}"),
            kind: ReasonKind::Install,
            label: clean_label(title, "an install"),
        }
    }

    pub fn verify(game_id: i64, title: &str) -> Self {
        Self {
            key: format!("verify:{game_id}"),
            kind: ReasonKind::Verify,
            label: clean_label(title, "an install"),
        }
    }

    /// Repairing an install transfers missing files back into the folder.
    pub fn repair(game_id: i64, title: &str) -> Self {
        Self {
            key: format!("repair:{game_id}"),
            kind: ReasonKind::Install,
            label: clean_label(title, "an install"),
        }
    }

    pub fn game(session_id: &str, title: &str) -> Self {
        Self {
            key: format!("game:{session_id}"),
            kind: ReasonKind::Game,
            label: clean_label(title, "a game"),
        }
    }

    pub fn stream(session_id: &str, app: &str) -> Self {
        Self {
            key: format!("stream:{session_id}"),
            kind: ReasonKind::Stream,
            label: clean_label(app, "a stream"),
        }
    }
}

/// The serialisable view of one reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonView {
    pub kind: ReasonKind,
    pub label: String,
}

/* ------------------------------ state machine ----------------------------- */

/// What a mutation changed. The runtime only uses it to decide whether to wake
/// the supervisor, but it keeps the machine testable without a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    /// A reason arrived while the inhibitor was not held (or the grace window
    /// was pending): the lock must be (re)acquired now.
    Started,
    /// Another reason joined an already held inhibitor.
    Extended,
    /// The last reason ended and the grace window is running.
    Scheduled,
    /// The last reason ended with no grace: the lock must be released now.
    Released,
}

/// The pure inhibitor state: a keyed reason set plus the grace deadline.
///
/// `Instant` is passed in, never read here, so every transition is
/// deterministic in a test.
#[derive(Debug, Clone)]
pub struct InhibitMachine {
    reasons: BTreeMap<String, Reason>,
    grace: Duration,
    /// When the reason set last became empty; `None` while it is non-empty or
    /// once the grace window expired (`tick`).
    ended_at: Option<Instant>,
}

impl InhibitMachine {
    pub fn new(grace: Duration) -> Self {
        Self {
            reasons: BTreeMap::new(),
            grace,
            ended_at: None,
        }
    }

    /// Change the window. A pending deadline moves with it (a shorter value can
    /// expire at the next tick, and `0` releases immediately).
    pub fn set_grace(&mut self, grace: Duration) {
        self.grace = grace;
    }

    pub fn grace_minutes(&self) -> u32 {
        u32::try_from(self.grace.as_secs() / 60).unwrap_or(MAX_GRACE_MINUTES)
    }

    pub fn add(&mut self, reason: Reason, _now: Instant) -> Change {
        if self.reasons.contains_key(&reason.key) {
            // Idempotent: the holder already owns its key.
            return Change::None;
        }
        let pending = self.ended_at.take().is_some();
        let was_empty = self.reasons.is_empty();
        self.reasons.insert(reason.key.clone(), reason);
        if was_empty || pending {
            Change::Started
        } else {
            Change::Extended
        }
    }

    pub fn remove(&mut self, key: &str, now: Instant) -> Change {
        if self.reasons.remove(key).is_none() {
            return Change::None;
        }
        if !self.reasons.is_empty() {
            return Change::None;
        }
        if self.grace.is_zero() {
            self.ended_at = None;
            Change::Released
        } else {
            self.ended_at = Some(now);
            Change::Scheduled
        }
    }

    /// Expire the grace window once its deadline passed.
    pub fn tick(&mut self, now: Instant) -> Change {
        if !self.reasons.is_empty() {
            return Change::None;
        }
        if let Some(end) = self.ended_at {
            if now >= end + self.grace {
                self.ended_at = None;
                return Change::Released;
            }
        }
        Change::None
    }

    /// True while the inhibitor must be held: a reason is active, or the grace
    /// window has not expired yet.
    pub fn holding_at(&self, now: Instant) -> bool {
        if !self.reasons.is_empty() {
            return true;
        }
        self.ended_at
            .map(|end| now < end + self.grace)
            .unwrap_or(false)
    }

    /// When the supervisor must wake up again, if nothing else happens first.
    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.reasons.is_empty() {
            return None;
        }
        self.ended_at.map(|end| end + self.grace)
    }

    pub fn count(&self) -> u32 {
        u32::try_from(self.reasons.len()).unwrap_or(u32::MAX)
    }

    /// Install-ish reasons, the "<n> jobs" of the banner.
    pub fn jobs(&self) -> u32 {
        let count = self
            .reasons
            .values()
            .filter(|reason| matches!(reason.kind, ReasonKind::Install | ReasonKind::Verify))
            .count();
        u32::try_from(count).unwrap_or(u32::MAX)
    }

    pub fn active(&self) -> bool {
        !self.reasons.is_empty()
    }

    pub fn views(&self) -> Vec<ReasonView> {
        let mut reasons: Vec<&Reason> = self.reasons.values().collect();
        reasons.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.key.cmp(&right.key))
        });
        reasons
            .into_iter()
            .map(|reason| ReasonView {
                kind: reason.kind,
                label: reason.label.clone(),
            })
            .collect()
    }

    /// The `--why=` text shown by `systemd-inhibit --list`, in a stable order.
    pub fn why(&self) -> String {
        let first = |kind: ReasonKind| -> Vec<&Reason> {
            self.reasons
                .values()
                .filter(|reason| reason.kind == kind)
                .collect()
        };
        let mut parts: Vec<String> = Vec::new();

        let installs = first(ReasonKind::Install);
        if let Some(first_install) = installs.first() {
            parts.push(if installs.len() == 1 {
                format!("downloading {}", first_install.label)
            } else {
                format!(
                    "downloading {} ({} jobs)",
                    first_install.label,
                    installs.len()
                )
            });
        }
        if let Some(verify) = first(ReasonKind::Verify).first() {
            parts.push(format!("checking {}", verify.label));
        }
        if let Some(game) = first(ReasonKind::Game).first() {
            parts.push(format!("playing {}", game.label));
        }
        if let Some(stream) = first(ReasonKind::Stream).first() {
            parts.push(format!("streaming {}", stream.label));
        }

        if parts.is_empty() {
            "Ludex is working".to_string()
        } else {
            format!("Ludex is {}", parts.join(", "))
        }
    }
}

/* --------------------------------- status --------------------------------- */

/// What the web shell and the diagnostics view show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InhibitStatus {
    /// An inhibitor can be held on this machine (`systemd-inhibit` exists).
    pub supported: bool,
    /// At least one reason is active right now.
    pub active: bool,
    /// The child process (or the grace window) currently keeps the machine up.
    pub holding: bool,
    /// Install-ish reasons: the "<n> jobs" of the banner.
    pub jobs: u32,
    /// Every active reason.
    pub count: u32,
    pub reasons: Vec<ReasonView>,
    pub grace_minutes: u32,
}

/* ------------------------------- the child -------------------------------- */

/// A held inhibitor. Dropping it must kill and reap the child.
pub trait Inhibitor: Send {
    fn alive(&mut self) -> bool;
}

/// Starts inhibitor children. The trait is what lets the state machine be
/// tested without a real logind.
pub trait Spawner: Send + Sync {
    fn available(&self) -> bool;
    fn start(&self, why: &str) -> Result<Box<dyn Inhibitor>, String>;
}

/// A spawner for a platform (or a build) that cannot hold an inhibitor.
///
/// Constructed on every non-Linux build and by the test-only default spawner;
/// on a Linux release build `LinuxSpawner` covers the same case through its
/// missing binary.
#[allow(dead_code)]
pub struct UnavailableSpawner;

impl Spawner for UnavailableSpawner {
    fn available(&self) -> bool {
        false
    }

    fn start(&self, _why: &str) -> Result<Box<dyn Inhibitor>, String> {
        Err("This machine has no suspend inhibitor.".to_string())
    }
}

/// The `systemd-inhibit` wrapper.
///
/// One child per hold, killed and reaped in [`SystemdInhibitor::release`] and
/// again from `Drop`. The `sleep infinity` grandchild is killed first (through
/// the shared process sampler), because a bare `systemd-inhibit` kill would
/// leave it behind.
pub struct SystemdInhibitor {
    binary: PathBuf,
    pid: u32,
    child: Child,
}

impl SystemdInhibitor {
    /// Spawn the wrapper with the exact argument contract of #288.
    pub fn start(binary: &Path, why: &str) -> Result<Self, String> {
        let mut child = Command::new(binary)
            .arg(format!("--what={WHAT}"))
            .arg("--mode=block")
            .arg(format!("--who={WHO}"))
            .arg(format!("--why={why}"))
            .arg("sleep")
            .arg("infinity")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start {}: {error}", binary.display()))?;
        let pid = child.id();
        std::thread::sleep(READY_PROBE);

        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                let detail = stderr.trim();
                Err(if detail.is_empty() {
                    format!("`{}` exited with {status}.", binary.display())
                } else {
                    format!("`{}` exited with {status}: {detail}", binary.display())
                })
            }
            Ok(None) => Ok(Self {
                binary: binary.to_path_buf(),
                pid,
                child,
            }),
            Err(error) => Err(format!("Could not watch `{}`: {error}", binary.display())),
        }
    }

    /// Kill the `sleep infinity` and the wrapper, then reap it.
    fn stop(&mut self) {
        // Children first: `systemd-inhibit` only waits on its command, so
        // killing it alone would orphan the hold.
        crate::process_tree::ProcessSampler::new().kill_tree(self.pid);
        let _ = self.child.kill();
        let _ = self.child.wait();
        tracing::debug!(pid = self.pid, binary = %self.binary.display(), "suspend inhibitor released");
    }
}

impl Inhibitor for SystemdInhibitor {
    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for SystemdInhibitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Resolve an executable on a `PATH` value without a `which` dependency.
pub fn resolve_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// The release-build spawner. A test build never constructs it: the default
/// spawner is [`UnavailableSpawner`] so no test can touch the real logind lock.
#[cfg(target_os = "linux")]
#[cfg_attr(test, allow(dead_code))]
struct LinuxSpawner {
    binary: Option<PathBuf>,
}

#[cfg(target_os = "linux")]
#[cfg_attr(test, allow(dead_code))]
impl LinuxSpawner {
    fn discover() -> Self {
        Self {
            binary: resolve_on_path(COMMAND, std::env::var_os("PATH").as_deref()),
        }
    }
}

#[cfg(target_os = "linux")]
impl Spawner for LinuxSpawner {
    fn available(&self) -> bool {
        self.binary.is_some()
    }

    fn start(&self, why: &str) -> Result<Box<dyn Inhibitor>, String> {
        let binary = self
            .binary
            .as_deref()
            .ok_or_else(|| "`systemd-inhibit` is not installed.".to_string())?;
        Ok(Box::new(SystemdInhibitor::start(binary, why)?))
    }
}

/* -------------------------------- the manager ------------------------------ */

struct Inner {
    machine: InhibitMachine,
    child: Option<Box<dyn Inhibitor>>,
    /// The supervisor is spawning right now; `status` must not wait on it.
    starting: bool,
    /// A start failed: do not retry in a tight loop while the same work runs.
    blocked: bool,
    shutdown: bool,
}

/// The process-wide holder: one supervisor thread reconciles the child with
/// the pure state machine, and every lifecycle hook only mutates the machine.
pub struct Manager {
    inner: Mutex<Inner>,
    signal: Condvar,
    spawner: Box<dyn Spawner>,
    holding: AtomicBool,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    notify: Box<dyn Fn(&InhibitStatus) + Send + Sync>,
}

impl Manager {
    pub fn start(
        grace: Duration,
        spawner: Box<dyn Spawner>,
        notify: Box<dyn Fn(&InhibitStatus) + Send + Sync>,
    ) -> Arc<Self> {
        let manager = Arc::new(Self {
            inner: Mutex::new(Inner {
                machine: InhibitMachine::new(grace),
                child: None,
                starting: false,
                blocked: false,
                shutdown: false,
            }),
            signal: Condvar::new(),
            spawner,
            holding: AtomicBool::new(false),
            thread: Mutex::new(None),
            notify,
        });
        let worker = Arc::clone(&manager);
        let handle = std::thread::Builder::new()
            .name("ludex-inhibit".to_string())
            .spawn(move || worker.run())
            .ok();
        if let Ok(mut slot) = manager.thread.lock() {
            *slot = handle;
        }
        manager
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn run(&self) {
        let mut inner = self.lock();
        loop {
            if inner.shutdown {
                break;
            }
            inner.machine.tick(Instant::now());
            let want = inner.machine.holding_at(Instant::now());

            if !want {
                let released = inner.child.take().is_some();
                inner.blocked = false;
                if released {
                    self.holding.store(false, Ordering::SeqCst);
                    tracing::info!("released the suspend inhibitor: no Ludex work is running");
                    drop(inner);
                    self.publish();
                    inner = self.lock();
                }
            } else if inner.child.is_none() && !inner.starting && !inner.blocked {
                if !self.spawner.available() {
                    inner.blocked = true;
                } else {
                    inner.starting = true;
                    let why = inner.machine.why();
                    let reasons = inner.machine.views();
                    drop(inner);
                    let started = self.spawner.start(&why);
                    inner = self.lock();
                    inner.starting = false;
                    match started {
                        Ok(child) => {
                            inner.child = Some(child);
                            self.holding.store(true, Ordering::SeqCst);
                            tracing::info!(
                                why = %why,
                                jobs = inner.machine.jobs(),
                                reasons = ?reasons,
                                "holding the suspend inhibitor"
                            );
                            drop(inner);
                            self.publish();
                            inner = self.lock();
                        }
                        Err(error) => {
                            inner.blocked = true;
                            tracing::warn!(
                                error = %error,
                                "the suspend inhibitor could not start; this machine may sleep while Ludex works"
                            );
                            drop(inner);
                            self.publish();
                            inner = self.lock();
                        }
                    }
                }
            } else if let Some(child) = inner.child.as_mut() {
                if !child.alive() {
                    tracing::warn!("the suspend inhibitor exited on its own; not restarting it");
                    inner.child = None;
                    inner.blocked = true;
                    self.holding.store(false, Ordering::SeqCst);
                    drop(inner);
                    self.publish();
                    inner = self.lock();
                }
            }

            if inner.shutdown {
                break;
            }
            let timeout = inner
                .machine
                .next_deadline()
                .map(|deadline| deadline.saturating_duration_since(Instant::now()));
            inner = match timeout {
                Some(duration) => self
                    .signal
                    .wait_timeout(inner, duration)
                    .map(|(guard, _)| guard)
                    .unwrap_or_else(|poisoned| poisoned.into_inner().0),
                None => self
                    .signal
                    .wait(inner)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
        }

        // Never leave a stale inhibitor behind: `Drop` kills the wrapper and
        // the `sleep infinity`, and `wait` reaps the zombie.
        let released = inner.child.take().is_some();
        self.holding.store(false, Ordering::SeqCst);
        drop(inner);
        if released {
            tracing::info!("released the suspend inhibitor on exit");
            self.publish();
        }
    }

    fn publish(&self) {
        let status = self.status();
        (self.notify)(&status);
    }

    pub fn status(&self) -> InhibitStatus {
        let inner = self.lock();
        InhibitStatus {
            supported: self.spawner.available(),
            active: inner.machine.active(),
            holding: self.holding.load(Ordering::SeqCst),
            jobs: inner.machine.jobs(),
            count: inner.machine.count(),
            reasons: inner.machine.views(),
            grace_minutes: inner.machine.grace_minutes(),
        }
    }

    pub fn set_grace(&self, grace: Duration) {
        {
            let mut inner = self.lock();
            inner.machine.set_grace(grace);
        }
        self.signal.notify_all();
    }

    pub fn hold(self: &Arc<Self>, reason: Reason) -> HoldGuard {
        let key = reason.key.clone();
        let change = {
            let mut inner = self.lock();
            inner.machine.add(reason, Instant::now())
        };
        if change != Change::None {
            self.signal.notify_all();
        }
        HoldGuard {
            manager: Some(Arc::clone(self)),
            key,
        }
    }

    pub fn release(&self, key: &str) {
        let change = {
            let mut inner = self.lock();
            inner.machine.remove(key, Instant::now())
        };
        if change != Change::None {
            self.signal.notify_all();
        }
    }

    /// Release everything and join the supervisor. Idempotent; only the app
    /// exit hook calls it.
    pub fn shutdown(&self) {
        {
            let mut inner = self.lock();
            inner.shutdown = true;
        }
        self.signal.notify_all();
        let handle = self.thread.lock().ok().and_then(|mut slot| slot.take());
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

/// Releases its reason when dropped, so every early return and every panic
/// unwind in a job releases the inhibitor.
pub struct HoldGuard {
    manager: Option<Arc<Manager>>,
    key: String,
}

impl Drop for HoldGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.take() {
            manager.release(&self.key);
        }
    }
}

/* --------------------------------- global --------------------------------- */

static MANAGER: OnceLock<Arc<Manager>> = OnceLock::new();
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

#[cfg(all(not(test), target_os = "linux"))]
fn default_spawner() -> Box<dyn Spawner> {
    Box::new(LinuxSpawner::discover())
}

#[cfg(all(not(test), not(target_os = "linux")))]
fn default_spawner() -> Box<dyn Spawner> {
    Box::new(UnavailableSpawner)
}

/// Tests must never touch the real logind lock.
#[cfg(test)]
fn default_spawner() -> Box<dyn Spawner> {
    Box::new(UnavailableSpawner)
}

fn publish_to_app(status: &InhibitStatus) {
    use tauri::Emitter;
    if let Some(app) = APP.get() {
        if let Err(error) = app.emit("inhibit://status", status.clone()) {
            tracing::debug!(error = %error, "could not emit the inhibit status");
        }
    }
}

fn global() -> &'static Arc<Manager> {
    MANAGER.get_or_init(|| {
        Manager::start(
            grace_duration(DEFAULT_GRACE_MINUTES),
            default_spawner(),
            Box::new(publish_to_app),
        )
    })
}

/// Install the app handle (for status events) and the saved grace window.
pub fn init(app: &tauri::AppHandle, minutes: u32) {
    let _ = APP.set(app.clone());
    set_grace_minutes(minutes);
}

/// Apply the setting to the live manager. Called by the settings save and at
/// startup.
pub fn set_grace_minutes(minutes: u32) {
    global().set_grace(grace_duration(minutes));
}

pub fn status() -> InhibitStatus {
    global().status()
}

/// Hold the inhibitor for `reason` until the guard is dropped.
pub fn hold(reason: Reason) -> HoldGuard {
    global().hold(reason)
}

/// Release everything and stop the supervisor. Called from `RunEvent::Exit`.
pub fn shutdown() {
    global().shutdown();
}

/* ---------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn install(key: &str, title: &str) -> Reason {
        Reason {
            key: key.to_string(),
            kind: ReasonKind::Install,
            label: title.to_string(),
        }
    }

    fn game(key: &str, title: &str) -> Reason {
        Reason {
            key: key.to_string(),
            kind: ReasonKind::Game,
            label: title.to_string(),
        }
    }

    fn poll_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        check()
    }

    #[test]
    fn reasons_are_keyed_and_dedupe() {
        let now = Instant::now();
        let mut machine = InhibitMachine::new(Duration::ZERO);

        assert_eq!(
            machine.add(install("install:1", "Hades II"), now),
            Change::Started
        );
        assert_eq!(
            machine.add(install("install:1", "Hades II"), now),
            Change::None
        );
        assert_eq!(machine.count(), 1);

        assert_eq!(
            machine.add(install("install:2", "Celeste"), now),
            Change::Extended
        );
        assert_eq!(machine.count(), 2);
        assert_eq!(machine.jobs(), 2);

        assert_eq!(machine.remove("install:1", now), Change::None);
        assert!(machine.holding_at(now), "one reason still holds the lock");
        assert_eq!(machine.remove("install:2", now), Change::Released);
        assert!(!machine.holding_at(now));
        // A second release of the same key is a no-op.
        assert_eq!(machine.remove("install:2", now), Change::None);
    }

    #[test]
    fn the_last_release_waits_for_the_grace_window() {
        let start = Instant::now();
        let mut machine = InhibitMachine::new(Duration::from_secs(5 * 60));
        machine.add(install("install:1", "Hades II"), start);
        machine.add(game("game:a", "Celeste"), start);

        assert_eq!(machine.remove("install:1", start), Change::None);
        assert_eq!(
            machine.remove("game:a", start),
            Change::Scheduled,
            "the last release starts the grace window"
        );
        assert!(machine.holding_at(start + Duration::from_secs(60)));
        assert!(machine.holding_at(start + Duration::from_secs(4 * 60)));
        assert_eq!(
            machine.tick(start + Duration::from_secs(5 * 60 - 1)),
            Change::None
        );
        assert!(!machine.holding_at(start + Duration::from_secs(5 * 60)));
        assert_eq!(
            machine.tick(start + Duration::from_secs(5 * 60)),
            Change::Released
        );
        assert!(machine.next_deadline().is_none());
    }

    #[test]
    fn zero_grace_releases_immediately() {
        let now = Instant::now();
        let mut machine = InhibitMachine::new(Duration::ZERO);
        machine.add(install("install:1", "Hades II"), now);
        assert_eq!(machine.remove("install:1", now), Change::Released);
        assert!(!machine.holding_at(now));
        assert!(machine.next_deadline().is_none());
    }

    #[test]
    fn a_new_reason_cancels_the_grace_window() {
        let start = Instant::now();
        let mut machine = InhibitMachine::new(Duration::from_secs(5 * 60));
        machine.add(install("install:1", "Hades II"), start);
        machine.remove("install:1", start);
        assert!(machine.holding_at(start + Duration::from_secs(30)));

        let later = start + Duration::from_secs(30);
        assert_eq!(
            machine.add(game("game:a", "Celeste"), later),
            Change::Started
        );
        assert!(
            machine.next_deadline().is_none(),
            "the grace window is gone"
        );
        assert_eq!(machine.remove("game:a", later), Change::Scheduled);
    }

    #[test]
    fn the_why_text_names_every_kind_in_a_stable_order() {
        let now = Instant::now();
        let mut machine = InhibitMachine::new(Duration::ZERO);
        assert_eq!(machine.why(), "Ludex is working");

        machine.add(install("install:1", "Hades II"), now);
        assert_eq!(machine.why(), "Ludex is downloading Hades II");

        machine.add(install("install:2", "Celeste"), now);
        assert_eq!(machine.why(), "Ludex is downloading Hades II (2 jobs)");

        machine.add(Reason::verify(7, "Hades II"), now);
        machine.add(game("game:a", "Hollow Knight"), now);
        machine.add(Reason::stream("s1", "Hades II"), now);
        assert_eq!(
            machine.why(),
            "Ludex is downloading Hades II (2 jobs), checking Hades II, playing Hollow Knight, streaming Hades II"
        );
    }

    #[test]
    fn the_status_counts_jobs_separately_from_every_reason() {
        let now = Instant::now();
        let mut machine = InhibitMachine::new(Duration::from_secs(5 * 60));
        machine.add(install("install:1", "Hades II"), now);
        machine.add(install("install:2", "Celeste"), now);
        machine.add(game("game:a", "Hollow Knight"), now);

        assert_eq!(machine.count(), 3);
        assert_eq!(machine.jobs(), 2);
        assert!(machine.active());
        assert_eq!(
            machine.views(),
            vec![
                ReasonView {
                    kind: ReasonKind::Install,
                    label: "Hades II".to_string()
                },
                ReasonView {
                    kind: ReasonKind::Install,
                    label: "Celeste".to_string()
                },
                ReasonView {
                    kind: ReasonKind::Game,
                    label: "Hollow Knight".to_string()
                },
            ]
        );
    }

    #[test]
    fn an_empty_label_falls_back_to_a_sentence() {
        let reason = Reason::install("install:1", "   ");
        assert_eq!(reason.label, "an install");
        let reason = Reason::game("game:a", "");
        assert_eq!(reason.label, "a game");
        let reason = Reason::stream("s1", "");
        assert_eq!(reason.label, "a stream");
    }

    #[test]
    fn the_status_serialises_as_camel_case() {
        let status = InhibitStatus {
            supported: true,
            active: true,
            holding: true,
            jobs: 1,
            count: 2,
            reasons: vec![
                ReasonView {
                    kind: ReasonKind::Install,
                    label: "Hades II".to_string(),
                },
                ReasonView {
                    kind: ReasonKind::Stream,
                    label: "Hades II".to_string(),
                },
            ],
            grace_minutes: 5,
        };
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({
                "supported": true,
                "active": true,
                "holding": true,
                "jobs": 1,
                "count": 2,
                "reasons": [
                    { "kind": "install", "label": "Hades II" },
                    { "kind": "stream", "label": "Hades II" }
                ],
                "graceMinutes": 5
            })
        );
    }

    /* ------------------------- the real wrapper ------------------------- */

    #[cfg(target_os = "linux")]
    mod wrapper {
        use super::*;
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        struct Fake {
            dir: PathBuf,
            args: PathBuf,
            pid: PathBuf,
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        /// A fake `systemd-inhibit` on PATH: records its argv and pid, then
        /// waits on a `sleep` child so the process tree looks real.
        fn fake_inhibitor() -> Fake {
            let dir = std::env::temp_dir().join(format!(
                "ludex-inhibit-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let args = dir.join("args");
            let pid = dir.join("pid");
            let script = dir.join("systemd-inhibit");
            let mut file = std::fs::File::create(&script).unwrap();
            writeln!(file, "#!/bin/sh").unwrap();
            writeln!(file, "printf '%s\\n' \"$@\" > '{}'", args.display()).unwrap();
            writeln!(file, "echo $$ > '{}'", pid.display()).unwrap();
            writeln!(file, "sleep 30").unwrap();
            writeln!(file, "printf done > '{}/done'", dir.display()).unwrap();
            drop(file);
            let mut permissions = std::fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions).unwrap();
            Fake { dir, args, pid }
        }

        fn read_lines(path: &Path) -> Vec<String> {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        #[test]
        fn the_wrapper_uses_the_contract_arguments() {
            let fake = fake_inhibitor();
            let binary = resolve_on_path("systemd-inhibit", Some(fake.dir.as_os_str()))
                .expect("the fake command is on the fake PATH");
            assert!(binary.is_file());

            let inhibitor = SystemdInhibitor::start(&binary, "Ludex is downloading Test Game")
                .expect("the fake wrapper starts");
            assert!(poll_until(Duration::from_secs(2), || fake.args.is_file()));
            assert_eq!(
                read_lines(&fake.args),
                vec![
                    "--what=idle:sleep:handle-lid-switch".to_string(),
                    "--mode=block".to_string(),
                    "--who=Ludex".to_string(),
                    "--why=Ludex is downloading Test Game".to_string(),
                    "sleep".to_string(),
                    "infinity".to_string(),
                ]
            );
            drop(inhibitor);
        }

        #[test]
        fn dropping_the_wrapper_kills_and_reaps_the_child_tree() {
            let fake = fake_inhibitor();
            let binary = resolve_on_path("systemd-inhibit", Some(fake.dir.as_os_str())).unwrap();
            let inhibitor =
                SystemdInhibitor::start(&binary, "Ludex is downloading Test Game").unwrap();
            assert!(poll_until(Duration::from_secs(2), || fake.pid.is_file()));
            let pid = std::fs::read_to_string(&fake.pid)
                .unwrap()
                .trim()
                .to_string();
            assert!(!pid.is_empty());

            drop(inhibitor);

            assert!(
                poll_until(Duration::from_secs(3), || !Path::new(&format!(
                    "/proc/{pid}"
                ))
                .exists()),
                "the wrapper and its sleep are gone"
            );
            // The fake never reached its post-sleep line: the tree was killed.
            assert!(!fake.dir.join("done").exists());
        }

        #[test]
        fn a_wrapper_that_exits_reports_its_stderr() {
            let dir = std::env::temp_dir().join(format!(
                "ludex-inhibit-fail-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let script = dir.join("systemd-inhibit");
            let mut file = std::fs::File::create(&script).unwrap();
            writeln!(file, "#!/bin/sh").unwrap();
            writeln!(file, "echo 'Failed to connect to bus: No medium found' >&2").unwrap();
            writeln!(file, "exit 1").unwrap();
            drop(file);
            let mut permissions = std::fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script, permissions).unwrap();

            let error = match SystemdInhibitor::start(&script, "Ludex is working") {
                Ok(_) => panic!("an exited wrapper must be an error"),
                Err(error) => error,
            };
            assert!(error.contains("No medium found"), "{error}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn resolve_on_path_ignores_a_missing_command() {
            let dir =
                std::env::temp_dir().join(format!("ludex-inhibit-empty-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            assert!(resolve_on_path("systemd-inhibit", Some(dir.as_os_str())).is_none());
            assert!(resolve_on_path("systemd-inhibit", None).is_none());
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /* ---------------------------- the manager --------------------------- */

    struct FakeInhibitor {
        stops: Arc<AtomicUsize>,
        alive: Arc<AtomicBool>,
    }

    impl Inhibitor for FakeInhibitor {
        fn alive(&mut self) -> bool {
            self.alive.load(Ordering::SeqCst)
        }
    }

    impl Drop for FakeInhibitor {
        fn drop(&mut self) {
            if self.alive.swap(false, Ordering::SeqCst) {
                self.stops.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct FakeSpawner {
        starts: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
        alive: Arc<AtomicBool>,
        available: bool,
    }

    impl Spawner for FakeSpawner {
        fn available(&self) -> bool {
            self.available
        }

        fn start(&self, _why: &str) -> Result<Box<dyn Inhibitor>, String> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.alive.store(true, Ordering::SeqCst);
            Ok(Box::new(FakeInhibitor {
                stops: Arc::clone(&self.stops),
                alive: Arc::clone(&self.alive),
            }))
        }
    }

    fn fake_manager(grace: Duration) -> (Arc<Manager>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let alive = Arc::new(AtomicBool::new(false));
        let manager = Manager::start(
            grace,
            Box::new(FakeSpawner {
                starts: Arc::clone(&starts),
                stops: Arc::clone(&stops),
                alive,
                available: true,
            }),
            Box::new(|_| {}),
        );
        (manager, starts, stops)
    }

    #[test]
    fn one_inhibitor_serves_overlapping_reasons_and_stops_after_the_last() {
        let (manager, starts, stops) = fake_manager(Duration::ZERO);
        let first = manager.hold(install("install:1", "Hades II"));
        assert!(poll_until(Duration::from_secs(3), || starts
            .load(Ordering::SeqCst)
            == 1));
        let second = manager.hold(install("install:2", "Celeste"));
        assert!(poll_until(Duration::from_secs(3), || {
            let status = manager.status();
            status.count == 2 && status.holding && status.jobs == 2
        }));
        assert_eq!(
            starts.load(Ordering::SeqCst),
            1,
            "one child for the lifetime"
        );

        drop(first);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(stops.load(Ordering::SeqCst), 0, "one reason still runs");

        drop(second);
        assert!(poll_until(Duration::from_secs(3), || stops
            .load(Ordering::SeqCst)
            == 1));
        assert!(poll_until(Duration::from_secs(3), || !manager
            .status()
            .holding));
        manager.shutdown();
    }

    #[test]
    fn a_restart_after_the_grace_window_holds_again() {
        let (manager, starts, _stops) = fake_manager(Duration::from_millis(120));
        {
            let _first = manager.hold(install("install:1", "Hades II"));
            assert!(poll_until(Duration::from_secs(3), || starts
                .load(Ordering::SeqCst)
                == 1));
        }
        assert!(poll_until(Duration::from_secs(3), || !manager
            .status()
            .holding));
        {
            let _second = manager.hold(install("install:2", "Celeste"));
            assert!(poll_until(Duration::from_secs(3), || starts
                .load(Ordering::SeqCst)
                == 2));
        }
        manager.shutdown();
    }

    #[test]
    fn an_unsupported_machine_never_starts_a_child() {
        let starts = Arc::new(AtomicUsize::new(0));
        let alive = Arc::new(AtomicBool::new(false));
        let manager = Manager::start(
            Duration::ZERO,
            Box::new(FakeSpawner {
                starts: Arc::clone(&starts),
                stops: Arc::new(AtomicUsize::new(0)),
                alive,
                available: false,
            }),
            Box::new(|_| {}),
        );
        let guard = manager.hold(install("install:1", "Hades II"));
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        let status = manager.status();
        assert!(!status.supported);
        assert!(status.active);
        assert!(!status.holding);
        drop(guard);
        manager.shutdown();
    }

    #[test]
    fn shutdown_releases_a_held_inhibitor() {
        let (manager, starts, stops) = fake_manager(Duration::from_secs(600));
        let _guard = manager.hold(install("install:1", "Hades II"));
        assert!(poll_until(Duration::from_secs(3), || starts
            .load(Ordering::SeqCst)
            == 1));
        manager.shutdown();
        assert_eq!(stops.load(Ordering::SeqCst), 1);
        assert!(!manager.status().holding);
    }

    #[test]
    fn grace_minutes_are_bounded() {
        assert_eq!(clamp_grace_minutes(0), 0);
        assert_eq!(clamp_grace_minutes(5), 5);
        assert_eq!(clamp_grace_minutes(60), 60);
        assert_eq!(clamp_grace_minutes(61), 60);
        assert_eq!(clamp_grace_minutes(u32::MAX), 60);
    }

    #[test]
    fn the_setting_reaches_the_machine() {
        let (manager, _starts, _stops) = fake_manager(Duration::from_secs(60));
        assert_eq!(manager.status().grace_minutes, 1);
        manager.set_grace(grace_duration(30));
        assert_eq!(manager.status().grace_minutes, 30);
        manager.set_grace(grace_duration(600));
        assert_eq!(manager.status().grace_minutes, 60);
        manager.shutdown();
    }
}
