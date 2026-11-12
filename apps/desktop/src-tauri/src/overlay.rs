//! In-game controller overlay (#144).
//!
//! Two independent layers:
//!
//! 1. **In-app overlay (phase 1).** The main webview renders the overlay on
//!    Shift+Tab while Ludex is focused. Works everywhere, including macOS and
//!    in a plain browser preview.
//! 2. **Global overlay (phase 2).** A native gamepad listener reads buttons
//!    through `gilrs` even when another window (a game) owns the input, and a
//!    separate always-on-top transparent window shows the overlay above a
//!    borderless/windowed-fullscreen game. No DLL injection, no process hooks,
//!    and the game is never paused: Ludex only draws a second window.
//!
//! Limitations are documented in `apps/desktop/README.md` and
//! `docs/CONTRACT.md`: exclusive-fullscreen games generally hide every
//! non-injected overlay, Windows XInput/WGI does not expose the Guide button
//! (the LB+RB+Start chord is the fallback), and Linux focus restoration is
//! best-effort through `xdotool` on X11.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::db;
use crate::settings::{LocalSettings, OverlayTrigger};
use crate::state::AppState;

pub mod detector;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod global_listener;

/// Window label of the floating overlay. Also listed in `capabilities/default.json`.
pub const OVERLAY_LABEL: &str = "overlay";

/// Keyboard shortcut handled by the web UI while Ludex is focused.
pub const KEYBOARD_SHORTCUT: &str = "Shift+Tab";

/// How the overlay is currently presented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OverlayPresentation {
    #[default]
    Closed,
    /// A second always-on-top window, above the game (#144 phase 2).
    Window,
    /// Drawn inside the main window because the floating window is unavailable
    /// (unsupported platform or a window-creation failure).
    InApp,
}

/// Host platform, kept as a value so the trigger/capability rules are pure and
/// unit-tested instead of branching on `cfg!` inside every message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPlatform {
    Windows,
    Linux,
    MacOs,
    Other,
}

impl OverlayPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Other
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Other => "other",
        }
    }

    /// A second always-on-top window can be drawn above the game (#144 phase 2).
    pub const fn supports_floating_window(self) -> bool {
        matches!(self, Self::Windows | Self::Linux)
    }

    /// `gilrs` can read controllers while another window (the game) owns the
    /// keyboard/mouse focus.
    pub const fn supports_global_listener(self) -> bool {
        matches!(self, Self::Windows | Self::Linux)
    }

    /// The Guide/Home/PS button reaches the process. Windows XInput/WGI never
    /// reports it, which is why the LB+RB+Start chord is the fallback there.
    pub const fn supports_guide_button(self) -> bool {
        matches!(self, Self::Linux)
    }
}

/// One controller as reported by the native listener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayController {
    /// Process-local id from `gilrs`; stable while the pad stays connected.
    pub id: String,
    pub name: String,
    /// True for the pad that produced input most recently.
    pub active: bool,
    /// Battery percentage when the host reports it.
    pub battery: Option<u8>,
    pub charging: bool,
}

/// Native listener state, shown in the overlay footer and in Settings.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayListenerStatus {
    /// `gilrs` on Windows/Linux, `unsupported` elsewhere, `failed` after an init error.
    pub backend: String,
    /// True once `gilrs` initialised and controllers can be read.
    pub available: bool,
    /// True while the listener thread is alive. A thread that is retrying a
    /// failed `gilrs` initialisation keeps this true so the panel can tell a
    /// dead listener from a driver/permission error.
    pub running: bool,
    pub platform: String,
    pub reason: Option<String>,
    pub controllers: Vec<OverlayController>,
}

/// What a static capability query says about this build/host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayCapabilities {
    pub platform: String,
    /// A separate always-on-top window exists on this platform.
    pub floating_window: bool,
    /// Windows and X11 can keep that window above a borderless game; the
    /// Wayland compositor decides and is documented as best-effort.
    pub always_on_top: bool,
    /// The native controller listener can run here.
    pub global_listener: bool,
    /// The Guide/Home/PS button reaches the process.
    pub guide_button: bool,
    /// The LB+RB+Start chord is readable.
    pub chord: bool,
}

/// How much of the configured trigger can actually fire right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayTriggerStatus {
    pub trigger: OverlayTrigger,
    /// The double-Home gesture is part of the configuration.
    pub guide_enabled: bool,
    /// The LB+RB+Start chord is part of the configuration.
    pub chord_enabled: bool,
    /// Gesture that will actually fire first: `guide`, `chord`, `keyboard` or
    /// `none` when no controller gesture can be delivered.
    pub effective: String,
    /// True when at least one configured controller gesture can fire.
    pub deliverable: bool,
    /// Stable code the settings panel translates (`guide-unsupported-windows`,
    /// `no-controller`, `listener-unavailable`, `trigger-keyboard`, ...).
    pub note_code: Option<String>,
    /// English fallback for logs and older web bundles.
    pub note: Option<String>,
}

/// Fullscreen state of the foreground window, best effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum OverlayFullscreen {
    /// No foreground window, or the platform does not expose its style.
    #[default]
    Unknown,
    /// Decorated window, or a window that does not cover the monitor.
    Windowed,
    /// Undecorated window covering the monitor. Windows style bits cannot tell
    /// borderless from exclusive fullscreen without DXGI, so the UI explains
    /// the exclusive risk instead of pretending to know.
    Fullscreen,
}

/// Foreground window that would receive the overlay, as far as the OS reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayForeground {
    pub title: Option<String>,
    pub fullscreen: OverlayFullscreen,
}

/// One line of the "why does it not open" checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayCheck {
    /// Stable code the settings panel translates.
    pub id: String,
    /// `ok`, `info`, `warning` or `blocking`.
    pub level: String,
    /// English fallback; the panel translates the id when it knows it.
    pub message: String,
}

pub const CHECK_OK: &str = "ok";
pub const CHECK_INFO: &str = "info";
pub const CHECK_WARNING: &str = "warning";
pub const CHECK_BLOCKING: &str = "blocking";

/// Status decision tree result: each failure reason is one check, so the panel
/// can explain a refused trigger instead of doing nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayDiagnostics {
    /// The overlay can be opened right now (feature switch + per-game opt-out).
    pub can_open: bool,
    /// At least one configured controller gesture can fire.
    pub trigger_deliverable: bool,
    /// A floating window can be drawn above the running game.
    pub over_game: bool,
    /// First blocking check id, else the first warning, else `ready`.
    pub summary: String,
    pub checks: Vec<OverlayCheck>,
}

/// Last trigger/open/close attempt, so the panel can show whether presses
/// arrive at all and why an attempt was ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayTriggerEvent {
    /// RFC 3339 timestamp.
    pub at: String,
    /// `controller`, `manual` or `settings`.
    pub source: String,
    /// `open` or `close`.
    pub action: String,
    /// `openedWindow`, `openedInApp`, `closed` or `ignored`.
    pub outcome: String,
    /// Reason for an ignored attempt, or the window-creation error that forced
    /// the in-app fallback.
    pub detail: Option<String>,
}

/// The running game shown in the overlay.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayGameState {
    pub session_id: String,
    pub game_id: i64,
    pub title: String,
    /// Root pid of the launched game.
    pub pid: u32,
    pub started_at: String,
    /// Seconds since the launch, as of this status call.
    pub elapsed_seconds: i64,
    /// Finished sessions recorded before this launch, plus the current one.
    pub total_seconds: i64,
    /// True when the overlay is disabled for this game.
    pub disabled: bool,
}

/// Everything the overlay UI and the settings panel need in one value.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayStatus {
    pub enabled: bool,
    pub open: bool,
    pub trigger: OverlayTrigger,
    pub opacity: f64,
    pub scale: f64,
    pub tabs: Vec<String>,
    pub disabled_games: Vec<i64>,
    pub game: Option<OverlayGameState>,
    pub game_disabled: bool,
    pub presentation: OverlayPresentation,
    pub listener: OverlayListenerStatus,
    /// Static platform capabilities, so the panel can say what is possible
    /// before any controller is plugged in.
    pub capabilities: OverlayCapabilities,
    /// Whether the configured trigger can actually fire here.
    pub trigger_status: OverlayTriggerStatus,
    /// "Why does it not open" checklist.
    pub diagnostics: OverlayDiagnostics,
    /// Foreground window and fullscreen state while a game is running.
    pub foreground: Option<OverlayForeground>,
    /// Last trigger/open/close attempt.
    pub last_trigger: Option<OverlayTriggerEvent>,
    /// Human-readable limitation for this platform.
    pub platform_note: String,
    /// Keyboard shortcut that always works while the main window is focused.
    pub keyboard_shortcut: String,
}

/// Partial update from the settings panel; every field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OverlaySettingsPatch {
    pub enabled: Option<bool>,
    pub trigger: Option<OverlayTrigger>,
    pub opacity: Option<f64>,
    pub scale: Option<f64>,
    pub tabs: Option<Vec<String>>,
    pub disabled_games: Option<Vec<i64>>,
}

/// Per-platform return-focus token captured when the overlay opens.
#[cfg(windows)]
type FocusToken = isize;
#[cfg(all(unix, not(target_os = "macos")))]
type FocusToken = u64;
#[cfg(target_os = "macos")]
type FocusToken = ();

/// Runtime state shared between the commands, the web UI and the listener thread.
pub struct OverlayRuntime {
    open: AtomicBool,
    /// True while the listener thread is alive (including its retry loop).
    listener_running: AtomicBool,
    presentation: Mutex<OverlayPresentation>,
    listener: Mutex<OverlayListenerStatus>,
    last_trigger: Mutex<Option<OverlayTriggerEvent>>,
    focus_return: Mutex<Option<FocusToken>>,
}

impl Default for OverlayRuntime {
    fn default() -> Self {
        Self {
            open: AtomicBool::new(false),
            listener_running: AtomicBool::new(false),
            presentation: Mutex::new(OverlayPresentation::Closed),
            listener: Mutex::new(OverlayListenerStatus {
                backend: "none".to_string(),
                available: false,
                running: false,
                platform: std::env::consts::OS.to_string(),
                reason: Some("The native gamepad listener has not started.".to_string()),
                controllers: Vec::new(),
            }),
            last_trigger: Mutex::new(None),
            focus_return: Mutex::new(None),
        }
    }
}

impl OverlayRuntime {
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }

    fn set_open(&self, open: bool) {
        self.open.store(open, Ordering::SeqCst);
    }

    pub fn listener_running(&self) -> bool {
        self.listener_running.load(Ordering::SeqCst)
    }

    pub fn set_listener_running(&self, running: bool) {
        self.listener_running.store(running, Ordering::SeqCst);
    }

    fn set_presentation(&self, presentation: OverlayPresentation) {
        if let Ok(mut current) = self.presentation.lock() {
            *current = presentation;
        }
    }

    fn presentation(&self) -> OverlayPresentation {
        self.presentation
            .lock()
            .map(|current| *current)
            .unwrap_or_default()
    }

    fn listener_status(&self) -> OverlayListenerStatus {
        let mut status = self
            .listener
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default();
        status.running = self.listener_running();
        status
    }

    pub fn set_listener_status(&self, status: OverlayListenerStatus) {
        if let Ok(mut current) = self.listener.lock() {
            *current = status;
        }
    }

    /// Record the last open/close attempt for the settings checklist.
    pub fn record_trigger(&self, event: OverlayTriggerEvent) {
        if let Ok(mut slot) = self.last_trigger.lock() {
            *slot = Some(event);
        }
    }

    fn last_trigger(&self) -> Option<OverlayTriggerEvent> {
        self.last_trigger.lock().ok().and_then(|slot| slot.clone())
    }

    fn take_focus_token(&self) -> Option<FocusToken> {
        self.focus_return
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    fn store_focus_token(&self, token: Option<FocusToken>) {
        if let Ok(mut slot) = self.focus_return.lock() {
            *slot = token;
        }
    }
}

/* -------------------------------- startup -------------------------------- */

/// Start the native listener (no-op stub on unsupported platforms).
pub fn start(app: &AppHandle) {
    ensure_listener(app);
}

/// Idempotent listener start.
///
/// Called at boot and from every `overlay_status` read: a thread that could not
/// be spawned at startup (or a settings file written after the app booted)
/// must not leave the overlay permanently deaf until the next restart.
pub fn ensure_listener(app: &AppHandle) {
    let state = app.state::<AppState>();
    let platform = OverlayPlatform::current();
    if !platform.supports_global_listener() {
        if state.overlay.listener_status().backend != "unsupported" {
            state.overlay.set_listener_status(OverlayListenerStatus {
                backend: "unsupported".to_string(),
                available: false,
                running: false,
                platform: platform.as_str().to_string(),
                reason: Some(
                    "The global controller listener is only available on Windows and Linux. Use Shift+Tab while Ludex is focused."
                        .to_string(),
                ),
                controllers: Vec::new(),
            });
        }
        return;
    }

    // The thread keeps this flag set for its whole life, including its retry
    // loop, so a failed `gilrs` init is retried inside the thread and a failed
    // spawn is retried on the next status read.
    if state.overlay.listener_running() {
        return;
    }
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    global_listener::start(app.clone());
}

/// Static capability matrix for this build and host.
pub fn capabilities() -> OverlayCapabilities {
    let platform = OverlayPlatform::current();
    OverlayCapabilities {
        platform: platform.as_str().to_string(),
        floating_window: platform.supports_floating_window(),
        always_on_top: platform.supports_floating_window(),
        global_listener: platform.supports_global_listener(),
        guide_button: platform.supports_guide_button(),
        chord: platform.supports_global_listener(),
    }
}

/// Resolve the configured trigger into the gesture that can actually fire.
///
/// Pure on purpose: the guide/chord/keyboard decision is the part that was
/// silently wrong on Windows (the default `doubleHome` never fires there) and
/// must stay testable without a controller or a live listener.
pub fn trigger_support(
    trigger: OverlayTrigger,
    platform: OverlayPlatform,
    listener_available: bool,
    controller_count: usize,
) -> OverlayTriggerStatus {
    let guide_enabled = trigger == OverlayTrigger::DoubleHome;
    let chord_enabled = trigger != OverlayTrigger::Keyboard;
    let listener_ready =
        platform.supports_global_listener() && listener_available && controller_count > 0;

    let effective = if !listener_ready {
        "none"
    } else if guide_enabled && platform.supports_guide_button() {
        "guide"
    } else if chord_enabled {
        "chord"
    } else {
        "none"
    };

    let (note_code, note) = if trigger == OverlayTrigger::Keyboard {
        (
            Some("trigger-keyboard"),
            Some(
                "Only the keyboard shortcut is armed; no controller gesture will open the overlay."
                    .to_string(),
            ),
        )
    } else if !platform.supports_global_listener() {
        (
            Some("listener-unsupported"),
            Some(
                "Controller gestures are not available on this platform; use the keyboard shortcut."
                    .to_string(),
            ),
        )
    } else if !listener_available {
        (
            Some("listener-unavailable"),
            Some(
                "The controller listener is not running; controller gestures cannot fire."
                    .to_string(),
            ),
        )
    } else if controller_count == 0 {
        (
            Some("no-controller"),
            Some(
                "No controller is connected; connect a pad and press a button so the system registers it."
                    .to_string(),
            ),
        )
    } else if guide_enabled && !platform.supports_guide_button() {
        (
            Some("guide-unsupported-windows"),
            Some(
                "This platform does not deliver the Guide/Home button to apps. LB+RB+Start still opens the overlay; set the gesture to LB+RB+Start to make that explicit."
                    .to_string(),
            ),
        )
    } else {
        (None, None)
    };

    OverlayTriggerStatus {
        trigger,
        guide_enabled,
        chord_enabled,
        effective: effective.to_string(),
        deliverable: effective != "none",
        note_code: note_code.map(str::to_string),
        note,
    }
}

/// Inputs of the status decision tree. Every field is a value, so each failure
/// reason is unit-tested without a live window or controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayDiagnosisInput {
    pub enabled: bool,
    pub game_disabled: bool,
    pub has_game: bool,
    pub trigger: OverlayTrigger,
    pub platform: OverlayPlatform,
    pub listener_available: bool,
    pub listener_running: bool,
    pub listener_reason: Option<String>,
    pub controller_count: usize,
    pub floating_window: bool,
    pub fullscreen: OverlayFullscreen,
}

fn overlay_check(id: &str, level: &str, message: impl Into<String>) -> OverlayCheck {
    OverlayCheck {
        id: id.to_string(),
        level: level.to_string(),
        message: message.into(),
    }
}

/// Status decision tree: one check per reason the overlay would not open.
pub fn diagnose(input: &OverlayDiagnosisInput) -> OverlayDiagnostics {
    let mut checks = Vec::new();

    checks.push(if input.enabled {
        overlay_check("overlay-enabled", CHECK_OK, "The overlay is enabled.")
    } else {
        overlay_check(
            "overlay-disabled",
            CHECK_BLOCKING,
            "The overlay is turned off in Settings → Profiles.",
        )
    });

    if input.has_game {
        checks.push(if input.game_disabled {
            overlay_check(
                "game-disabled",
                CHECK_BLOCKING,
                "The overlay is turned off for this game.",
            )
        } else {
            overlay_check(
                "game-enabled",
                CHECK_OK,
                "The overlay is enabled for the running game.",
            )
        });
    }

    if !input.platform.supports_global_listener() {
        checks.push(overlay_check(
            "listener-unsupported",
            CHECK_INFO,
            "Controller gestures need Windows or Linux; use the keyboard shortcut.",
        ));
    } else if !input.listener_running {
        checks.push(overlay_check(
            "listener-stopped",
            CHECK_WARNING,
            "The controller listener thread is not running.",
        ));
    } else if input.listener_available {
        checks.push(overlay_check(
            "listener-running",
            CHECK_OK,
            format!(
                "The controller listener is running ({} controller(s)).",
                input.controller_count
            ),
        ));
    } else {
        checks.push(overlay_check(
            "listener-failed",
            CHECK_WARNING,
            input
                .listener_reason
                .clone()
                .unwrap_or_else(|| "The controller listener could not start.".to_string()),
        ));
    }

    if input.platform.supports_global_listener() && input.listener_available {
        checks.push(if input.controller_count == 0 {
            overlay_check(
                "no-controller",
                CHECK_WARNING,
                "No controller detected. Connect a pad and press a button so the system registers it; the keyboard shortcut still works.",
            )
        } else {
            overlay_check(
                "controller-connected",
                CHECK_OK,
                format!("{} controller(s) detected.", input.controller_count),
            )
        });
    }

    let trigger = trigger_support(
        input.trigger,
        input.platform,
        input.listener_available,
        input.controller_count,
    );
    match trigger.note_code.as_deref() {
        Some("trigger-keyboard") => checks.push(overlay_check(
            "trigger-keyboard",
            CHECK_INFO,
            "Only the keyboard shortcut is armed; no controller gesture opens the overlay.",
        )),
        Some("guide-unsupported-windows") => checks.push(overlay_check(
            "guide-unsupported-windows",
            CHECK_WARNING,
            "Windows does not deliver the Guide/Home button to apps. LB+RB+Start still opens the overlay, or switch the gesture to LB+RB+Start.",
        )),
        Some("no-controller") => checks.push(overlay_check(
            "trigger-no-controller",
            CHECK_WARNING,
            "The configured gesture cannot fire without a controller. The keyboard shortcut still works.",
        )),
        Some("listener-unavailable") | Some("listener-unsupported") => checks.push(overlay_check(
            "trigger-unavailable",
            CHECK_WARNING,
            "The configured gesture needs the controller listener, which is not available.",
        )),
        _ if trigger.deliverable => checks.push(overlay_check(
            "trigger-ready",
            CHECK_OK,
            "The configured controller gesture is ready.",
        )),
        _ => {}
    }

    if !input.has_game {
        checks.push(overlay_check(
            "no-game",
            CHECK_INFO,
            "No game is running; open the overlay from Settings or launch a game first.",
        ));
    } else {
        checks.push(match input.fullscreen {
            OverlayFullscreen::Windowed => overlay_check(
                "game-windowed",
                CHECK_OK,
                "The game window is windowed or borderless: the overlay can draw above it.",
            ),
            OverlayFullscreen::Fullscreen => overlay_check(
                "game-fullscreen",
                CHECK_WARNING,
                "The game covers the screen without window decorations. If it is in exclusive fullscreen, Windows hides every overlay that is not injected: switch the game to borderless or windowed fullscreen.",
            ),
            OverlayFullscreen::Unknown => overlay_check(
                "game-fullscreen-unknown",
                CHECK_INFO,
                "The game window mode could not be detected. If the overlay does not appear, switch the game to borderless or windowed fullscreen.",
            ),
        });
    }

    checks.push(if input.floating_window {
        overlay_check(
            "floating-window",
            CHECK_OK,
            "A floating always-on-top window can be drawn above a borderless game.",
        )
    } else {
        overlay_check(
            "no-floating-window",
            CHECK_WARNING,
            "This platform can only draw the overlay inside Ludex, not over a game.",
        )
    });

    let summary = checks
        .iter()
        .find(|check| check.level == CHECK_BLOCKING)
        .or_else(|| checks.iter().find(|check| check.level == CHECK_WARNING))
        .map(|check| check.id.clone())
        .unwrap_or_else(|| "ready".to_string());

    OverlayDiagnostics {
        can_open: input.enabled && !input.game_disabled,
        trigger_deliverable: trigger.deliverable,
        over_game: input.floating_window && input.fullscreen != OverlayFullscreen::Fullscreen,
        summary,
        checks,
    }
}

/// Platform limitation shown by the settings panel and the overlay.
pub const fn platform_note() -> &'static str {
    if cfg!(target_os = "windows") {
        "Borderless or windowed fullscreen is required: an exclusive-fullscreen game usually hides the overlay. Windows does not expose the Guide button through XInput/WGI, so the LB+RB+Start chord is the fallback. No injection or process hook is used."
    } else if cfg!(target_os = "linux") {
        "Borderless or windowed fullscreen is required: an exclusive-fullscreen game usually hides the overlay. Restoring focus to an X11 game needs xdotool; on Wayland the compositor decides whether the window stays on top and where focus returns. No injection or process hook is used."
    } else {
        "The floating overlay over a game and the global controller listener are available on Windows and Linux. Shift+Tab works while Ludex is focused."
    }
}

/* --------------------------------- status -------------------------------- */

/// The process entry that started most recently, as `(index, started_ms)`.
/// Kept as a pure helper so the selection rule is unit-tested without a child
/// process; `running_game` uses `max_by_key` on the same rule.
#[cfg(test)]
fn newest_started(started_at_ms: impl Iterator<Item = i64>) -> Option<usize> {
    let mut best: Option<(usize, i64)> = None;
    for (index, started) in started_at_ms.enumerate() {
        if best.map(|(_, value)| started > value).unwrap_or(true) {
            best = Some((index, started));
        }
    }
    best.map(|(index, _)| index)
}

fn is_game_disabled(disabled: &[i64], game_id: i64) -> bool {
    disabled.contains(&game_id)
}

struct RunningGame {
    session_id: String,
    game_id: i64,
    title: String,
    pid: u32,
    started_at: String,
    elapsed_seconds: i64,
    /// Playtime of finished sessions recorded before this launch.
    finished_seconds: i64,
}

/// Read the game currently running (the newest launch when several are).
fn running_game(state: &AppState) -> Option<RunningGame> {
    // Snapshot the process list under its lock, then release it before touching
    // the database: no nested locks between the process map and SQLite.
    let (session_id, game_id, pid, started_at, elapsed_seconds) = {
        let processes = state.processes.lock().ok()?;
        let (session_id, entry) = processes.iter().max_by_key(|(_, entry)| entry.started_at)?;
        (
            session_id.clone(),
            entry.game_id,
            entry.pid,
            entry
                .started_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            i64::try_from(entry.started.elapsed().as_secs()).unwrap_or(i64::MAX),
        )
    };

    let (title, finished_seconds) = {
        let Ok(connection) = state.db.lock() else {
            return Some(RunningGame {
                session_id,
                game_id,
                title: format!("Game {game_id}"),
                pid,
                started_at,
                elapsed_seconds,
                finished_seconds: 0,
            });
        };
        let title = db::get_installed(&connection, game_id)
            .ok()
            .flatten()
            .map(|installed| installed.title)
            .unwrap_or_else(|| format!("Game {game_id}"));
        let finished = db::game_playtime(&connection, game_id, 1, 1)
            .map(|playtime| playtime.total_seconds)
            .unwrap_or(0);
        (title, finished)
    };

    Some(RunningGame {
        session_id,
        game_id,
        title,
        pid,
        started_at,
        elapsed_seconds,
        finished_seconds,
    })
}

fn game_state(state: &AppState, settings: &LocalSettings) -> Option<OverlayGameState> {
    let game = running_game(state)?;
    Some(OverlayGameState {
        disabled: is_game_disabled(&settings.overlay_disabled_games, game.game_id),
        total_seconds: game.finished_seconds.saturating_add(game.elapsed_seconds),
        session_id: game.session_id,
        game_id: game.game_id,
        title: game.title,
        pid: game.pid,
        started_at: game.started_at,
        elapsed_seconds: game.elapsed_seconds,
    })
}

/// Broadcast the new overlay state to every window (and any future listener).
fn broadcast(app: &AppHandle, status: &OverlayStatus) {
    let _ = app.emit("overlay://state", status);
}

fn status_for(_app: &AppHandle, state: &AppState) -> OverlayStatus {
    let settings = state.settings_snapshot();
    let game = game_state(state, &settings);
    let listener = state.overlay.listener_status();
    let platform = OverlayPlatform::current();
    let capabilities = capabilities();
    let trigger_status = trigger_support(
        settings.overlay_trigger,
        platform,
        listener.available,
        listener.controllers.len(),
    );
    // Window-style probing is only meaningful while a game is running; it also
    // keeps `xprop` off the hot path when the app idles in the tray.
    let foreground = if game.is_some() {
        foreground_info()
    } else {
        None
    };
    let fullscreen = foreground
        .as_ref()
        .map(|info| info.fullscreen)
        .unwrap_or_default();
    let game_disabled = game.as_ref().map(|game| game.disabled).unwrap_or(false);
    let diagnostics = diagnose(&OverlayDiagnosisInput {
        enabled: settings.overlay_enabled,
        game_disabled,
        has_game: game.is_some(),
        trigger: settings.overlay_trigger,
        platform,
        listener_available: listener.available,
        listener_running: listener.running,
        listener_reason: listener.reason.clone(),
        controller_count: listener.controllers.len(),
        floating_window: capabilities.floating_window,
        fullscreen,
    });

    OverlayStatus {
        enabled: settings.overlay_enabled,
        open: state.overlay.is_open(),
        trigger: settings.overlay_trigger,
        opacity: settings.overlay_opacity,
        scale: settings.overlay_scale,
        tabs: settings.overlay_tabs.clone(),
        disabled_games: settings.overlay_disabled_games.clone(),
        game_disabled,
        game,
        presentation: state.overlay.presentation(),
        listener,
        capabilities,
        trigger_status,
        diagnostics,
        foreground,
        last_trigger: state.overlay.last_trigger(),
        platform_note: platform_note().to_string(),
        keyboard_shortcut: KEYBOARD_SHORTCUT.to_string(),
    }
}

/* ------------------------------ open / close ------------------------------ */

fn record_event(
    state: &AppState,
    source: &str,
    action: &str,
    outcome: &str,
    detail: Option<String>,
) {
    state.overlay.record_trigger(OverlayTriggerEvent {
        at: crate::util::now_rfc3339(),
        source: source.to_string(),
        action: action.to_string(),
        outcome: outcome.to_string(),
        detail,
    });
}

/// Open the overlay (idempotent). Refused when the feature or the game is off.
pub fn open(app: &AppHandle, source: &str) -> Result<OverlayStatus, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    tracing::info!(
        source,
        enabled = settings.overlay_enabled,
        trigger = ?settings.overlay_trigger,
        "overlay open requested"
    );

    if !settings.overlay_enabled {
        let reason =
            "The in-game overlay is turned off. Enable it in Settings → Profiles.".to_string();
        tracing::info!(source, reason = %reason, "overlay open ignored");
        record_event(&state, source, "open", "ignored", Some(reason.clone()));
        return Err(reason);
    }
    if let Some(game) = running_game(&state) {
        if is_game_disabled(&settings.overlay_disabled_games, game.game_id) {
            let reason = "The in-game overlay is turned off for this game.".to_string();
            tracing::info!(
                source,
                game_id = game.game_id,
                pid = game.pid,
                reason = %reason,
                "overlay open ignored"
            );
            record_event(&state, source, "open", "ignored", Some(reason.clone()));
            return Err(reason);
        }
    }

    // Idempotent: a second toggle while the overlay is already open must not
    // turn a floating window into an in-app panel (or the reverse).
    if state.overlay.is_open() {
        let presentation = state.overlay.presentation();
        if presentation == OverlayPresentation::Window {
            if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        tracing::info!(source, ?presentation, "overlay already open");
        return Ok(status_for(app, &state));
    }

    // Capture the foreground window before the overlay steals focus, so closing
    // it hands input back to the game (Windows: exact HWND; X11: xdotool).
    let focus_token = capture_foreground(app);
    tracing::info!(
        source,
        captured_foreground = focus_token.is_some(),
        "overlay opening"
    );
    state.overlay.store_focus_token(focus_token);
    state.overlay.set_open(true);

    // Phase 1: while Ludex itself is focused, draw the overlay inside the main
    // window. Phase 2: a game owns the focus, so a second always-on-top window
    // is required.
    let main_focused = app
        .get_webview_window("main")
        .map(|window| window.is_focused().unwrap_or(false))
        .unwrap_or(false);
    if main_focused {
        tracing::info!(
            source,
            "the main window is focused; using the in-app overlay"
        );
        state.overlay.set_presentation(OverlayPresentation::InApp);
        let _ = app.emit_to("main", "overlay://open", ());
        record_event(&state, source, "open", "openedInApp", None);
    } else if !capabilities().floating_window {
        tracing::warn!(
            source,
            platform = OverlayPlatform::current().as_str(),
            "no floating overlay window on this platform; using the in-app overlay"
        );
        state.overlay.set_presentation(OverlayPresentation::InApp);
        let _ = app.emit_to("main", "overlay://open", ());
        record_event(
            &state,
            source,
            "open",
            "openedInApp",
            Some("The floating overlay window is not available on this platform.".to_string()),
        );
    } else {
        match show_overlay_window(app) {
            Ok(()) => {
                let game = running_game(&state);
                tracing::info!(
                    source,
                    game_id = game.as_ref().map(|game| game.game_id),
                    pid = game.as_ref().map(|game| game.pid),
                    presentation = "window",
                    "overlay opened in the floating window"
                );
                state.overlay.set_presentation(OverlayPresentation::Window);
                record_event(&state, source, "open", "openedWindow", None);
            }
            Err(error) => {
                tracing::warn!(
                    source,
                    error = %error,
                    "the floating overlay window is unavailable; using the in-app overlay"
                );
                state.overlay.set_presentation(OverlayPresentation::InApp);
                let _ = app.emit_to("main", "overlay://open", ());
                record_event(&state, source, "open", "openedInApp", Some(error));
            }
        }
    }

    let status = status_for(app, &state);
    broadcast(app, &status);
    let _ = app.emit_to("overlay", "overlay://settings", &status);
    Ok(status)
}

/// Close the overlay and restore focus. Never touches the game process.
pub fn close(app: &AppHandle, source: &str) -> Result<OverlayStatus, String> {
    let state = app.state::<AppState>();
    let was_open = state.overlay.is_open();
    tracing::info!(source, was_open, "overlay close requested");
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
    }
    state.overlay.set_open(false);
    state.overlay.set_presentation(OverlayPresentation::Closed);
    let _ = app.emit_to("main", "overlay://close", ());
    restore_foreground(app, &state);
    record_event(&state, source, "close", "closed", None);
    let status = status_for(app, &state);
    broadcast(app, &status);
    Ok(status)
}

/// Listener entry point: open when closed, close when open.
pub fn toggle(app: &AppHandle, source: &str) {
    let state = app.state::<AppState>();
    let open_now = state.overlay.is_open();
    tracing::info!(source, open = open_now, "overlay toggle");
    let result = if open_now {
        close(app, source)
    } else {
        open(app, source)
    };
    if let Err(error) = result {
        // The refusal itself is logged with its reason by `open`.
        tracing::info!(source, error = %error, "overlay toggle ignored");
    }
}

fn show_overlay_window(app: &AppHandle) -> Result<(), String> {
    if !capabilities().floating_window {
        return Err("The floating overlay window is not available on this platform.".to_string());
    }

    let window = match app.get_webview_window(OVERLAY_LABEL) {
        Some(window) => window,
        None => {
            let builder = WebviewWindowBuilder::new(
                app,
                OVERLAY_LABEL,
                WebviewUrl::App("index.html?overlay=1".into()),
            )
            .title("Ludex overlay")
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .visible(false)
            .shadow(false)
            .focused(false)
            .inner_size(1280.0, 800.0);
            // Transparent windows need macOS private API, which Ludex does not
            // enable; the overlay is still usable there with an opaque panel.
            #[cfg(not(target_os = "macos"))]
            let builder = builder.transparent(true);
            let window = builder
                .build()
                .map_err(|error| format!("Could not create the overlay window: {error}"))?;
            tracing::info!(
                label = OVERLAY_LABEL,
                "created the floating overlay window on demand"
            );
            window
        }
    };

    position_on_current_monitor(app, &window);
    window
        .show()
        .map_err(|error| format!("Could not show the overlay window: {error}"))?;
    let _ = window.unminimize();
    // Some window managers drop the topmost flag when a hidden window is shown
    // again, so re-assert it on every open instead of only at creation.
    let _ = window.set_always_on_top(true);
    let _ = window.set_skip_taskbar(true);
    if let Err(error) = window.set_focus() {
        tracing::warn!(error = %error, "could not focus the overlay window; retrying once");
        window
            .set_focus()
            .map_err(|error| format!("Could not focus the overlay window: {error}"))?;
    }

    Ok(())
}

/// Cover the monitor the main window is on, or the primary monitor.
fn position_on_current_monitor(app: &AppHandle, window: &tauri::WebviewWindow) {
    let monitor = app
        .get_webview_window("main")
        .and_then(|main| main.current_monitor().ok().flatten())
        .or_else(|| {
            app.get_webview_window("main")
                .and_then(|main| main.primary_monitor().ok().flatten())
        });
    let Some(monitor) = monitor else {
        tracing::warn!("no monitor reported; the overlay keeps its default size");
        return;
    };
    let position = monitor.position();
    let size = monitor.size();
    if let Err(error) = window.set_position(tauri::PhysicalPosition::new(position.x, position.y)) {
        tracing::warn!(error = %error, "could not position the overlay window");
    }
    if let Err(error) = window.set_size(tauri::PhysicalSize::new(size.width, size.height)) {
        tracing::warn!(error = %error, "could not size the overlay window");
    }
}

/* -------------------------------- settings ------------------------------- */

fn apply_patch(settings: &mut LocalSettings, patch: OverlaySettingsPatch) {
    if let Some(enabled) = patch.enabled {
        settings.overlay_enabled = enabled;
    }
    if let Some(trigger) = patch.trigger {
        settings.overlay_trigger = trigger;
    }
    if let Some(opacity) = patch.opacity {
        settings.overlay_opacity = opacity;
    }
    if let Some(scale) = patch.scale {
        settings.overlay_scale = scale;
    }
    if let Some(tabs) = patch.tabs {
        settings.overlay_tabs = tabs;
    }
    if let Some(disabled) = patch.disabled_games {
        settings.overlay_disabled_games = disabled;
    }
}

/// Persist and apply an overlay settings change without a restart.
pub fn update_settings(
    app: &AppHandle,
    patch: OverlaySettingsPatch,
) -> Result<OverlayStatus, String> {
    let state = app.state::<AppState>();
    state.update_settings(|current| apply_patch(current, patch))?;
    let mut status = status_for(app, &state);

    // Live apply: a disabled overlay closes immediately, an open one picks up
    // the new opacity/scale/tabs through the emitted status.
    if !status.enabled && state.overlay.is_open() {
        status = close(app, "settings")?;
    }
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        if window.is_visible().unwrap_or(false) {
            position_on_current_monitor(app, &window);
        }
    }
    let _ = app.emit_to("overlay", "overlay://settings", &status);
    let _ = app.emit_to("main", "overlay://settings", &status);
    broadcast(app, &status);
    Ok(status)
}

/// Turn the overlay off (or back on) for one game.
pub fn set_game_disabled(
    app: &AppHandle,
    game_id: i64,
    disabled: bool,
) -> Result<OverlayStatus, String> {
    if game_id <= 0 {
        return Err("A game id is required.".to_string());
    }
    let state = app.state::<AppState>();
    state.update_settings(|current| {
        current
            .overlay_disabled_games
            .retain(|candidate| *candidate != game_id);
        if disabled {
            current.overlay_disabled_games.push(game_id);
        }
    })?;
    let mut status = status_for(app, &state);
    if disabled && status.game.as_ref().map(|game| game.game_id) == Some(game_id) {
        status = close(app, "settings")?;
    }
    let _ = app.emit_to("overlay", "overlay://settings", &status);
    Ok(status)
}

/* ------------------------------- big picture ------------------------------ */

/// Bring the main window forward on Big Picture and close the overlay.
pub fn show_big_picture(app: &AppHandle) -> Result<(), String> {
    let _ = close(app, "manual");
    crate::tray::show_window(app);
    app.emit_to("main", "overlay://navigate", "/bigpicture")
        .map_err(|error| format!("Could not reach the main window: {error}"))?;
    Ok(())
}

/* --------------------------------- focus --------------------------------- */

/// Where closing the overlay should send focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusReturn {
    /// The window captured before opening (the game, usually).
    Captured,
    /// Ludex itself, only when no game is running.
    MainWindow,
    /// Change nothing: the WM/game keeps its current focus.
    Leave,
}

/// Pure close-focus rule, so the "never steal focus from a running game"
/// promise is tested without a live window.
pub fn focus_return_plan(captured: bool, game_running: bool) -> FocusReturn {
    if captured {
        FocusReturn::Captured
    } else if game_running {
        FocusReturn::Leave
    } else {
        FocusReturn::MainWindow
    }
}

#[cfg(windows)]
fn capture_foreground(_app: &AppHandle) -> Option<FocusToken> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        None
    } else {
        Some(hwnd.0 as isize)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn capture_foreground(_app: &AppHandle) -> Option<FocusToken> {
    // Best effort on X11. Wayland compositors refuse to report another client's
    // window, and xdotool may not be installed: the overlay still closes, the
    // window manager then decides where focus goes.
    let output = crate::process::capture("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(target_os = "macos")]
fn capture_foreground(_app: &AppHandle) -> Option<FocusToken> {
    None
}

#[cfg(windows)]
fn restore_foreground(app: &AppHandle, state: &AppState) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

    let captured = state.overlay.take_focus_token();
    let plan = focus_return_plan(captured.is_some(), running_game(state).is_some());
    if let Some(hwnd) = captured {
        // Best effort: Windows may refuse foreground changes requested by a
        // background process, in which case the game keeps its own focus.
        unsafe {
            let _ = SetForegroundWindow(HWND(hwnd as *mut std::ffi::c_void));
        }
    }
    if plan == FocusReturn::MainWindow {
        crate::tray::show_window(app);
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn restore_foreground(app: &AppHandle, state: &AppState) {
    let captured = state.overlay.take_focus_token();
    let plan = focus_return_plan(captured.is_some(), running_game(state).is_some());
    if let Some(window_id) = captured {
        let restored = crate::process::command("xdotool")
            .args(["windowactivate", "--sync", &window_id.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !restored {
            tracing::debug!("xdotool could not restore the game window; leaving focus to the WM");
        }
    }
    if plan == FocusReturn::MainWindow {
        crate::tray::show_window(app);
    }
}

#[cfg(target_os = "macos")]
fn restore_foreground(app: &AppHandle, state: &AppState) {
    let captured = state.overlay.take_focus_token();
    let plan = focus_return_plan(captured.is_some(), running_game(state).is_some());
    if plan == FocusReturn::MainWindow {
        crate::tray::show_window(app);
    }
}

/* ------------------------------ foreground ------------------------------- */

/// Best-effort description of the window the overlay would cover.
///
/// Never used to decide whether to open: exclusive fullscreen cannot be told
/// apart from borderless fullscreen without DXGI, so the checklist warns the
/// user and the open path still tries. Returns `None` when the platform cannot
/// report the window, which the checklist renders as `unknown`.
#[cfg(windows)]
fn foreground_info() -> Option<OverlayForeground> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetSystemMetrics, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
        GetWindowTextW, GWL_STYLE, SM_CXSCREEN, SM_CYSCREEN, WS_CAPTION, WS_THICKFRAME,
    };

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }

    let mut rect = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return None;
    }

    let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
    let decorated = style & (WS_CAPTION.0 | WS_THICKFRAME.0) != 0;
    let width = rect.right.saturating_sub(rect.left);
    let height = rect.bottom.saturating_sub(rect.top);
    let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let covers_monitor =
        width >= screen_width && height >= screen_height && rect.left <= 0 && rect.top <= 0;
    let fullscreen = if decorated || !covers_monitor {
        OverlayFullscreen::Windowed
    } else {
        OverlayFullscreen::Fullscreen
    };

    let length = unsafe { GetWindowTextLengthW(hwnd) };
    let title = if length > 0 {
        let mut buffer = vec![0u16; usize::try_from(length + 1).unwrap_or(0)];
        let written = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if written > 0 {
            Some(String::from_utf16_lossy(
                &buffer[..usize::try_from(written).unwrap_or(0)],
            ))
        } else {
            None
        }
    } else {
        None
    };

    Some(OverlayForeground { title, fullscreen })
}

/// X11 granularity: `_NET_WM_STATE_FULLSCREEN` cannot separate borderless from
/// exclusive either, so both report `fullscreen`. Wayland refuses to report
/// another client's window and xprop may be absent: the checklist says unknown.
#[cfg(all(unix, not(target_os = "macos")))]
fn foreground_info() -> Option<OverlayForeground> {
    let active = crate::process::capture("xprop")
        .args(["-root", "_NET_ACTIVE_WINDOW"])
        .output()
        .ok()?;
    if !active.status.success() {
        return None;
    }
    let active = String::from_utf8_lossy(&active.stdout);
    let window_id = active.split('#').nth(1)?.trim().to_string();
    if window_id.is_empty() || window_id == "0x0" {
        return None;
    }

    let properties = crate::process::capture("xprop")
        .args(["-id", &window_id, "_NET_WM_STATE", "WM_NAME"])
        .output()
        .ok()?;
    if !properties.status.success() {
        return None;
    }
    let properties = String::from_utf8_lossy(&properties.stdout);
    let fullscreen = if properties.contains("_NET_WM_STATE_FULLSCREEN") {
        OverlayFullscreen::Fullscreen
    } else {
        OverlayFullscreen::Windowed
    };
    let title = properties
        .lines()
        .find(|line| line.starts_with("WM_NAME"))
        .and_then(|line| line.split_once('='))
        .map(|(_, value)| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty());

    Some(OverlayForeground { title, fullscreen })
}

#[cfg(target_os = "macos")]
fn foreground_info() -> Option<OverlayForeground> {
    None
}

/* -------------------------------- commands ------------------------------- */

#[tauri::command]
pub fn overlay_status(app: AppHandle, state: tauri::State<'_, AppState>) -> OverlayStatus {
    // Self-healing: a listener thread that failed to spawn at boot (or a
    // settings file loaded after startup) is retried on the next status read,
    // so the overlay cannot stay deaf until the next restart.
    ensure_listener(&app);
    status_for(&app, &state)
}

#[tauri::command]
pub fn set_overlay_settings(
    app: AppHandle,
    settings: OverlaySettingsPatch,
) -> Result<OverlayStatus, String> {
    update_settings(&app, settings)
}

#[tauri::command]
pub fn open_overlay(app: AppHandle) -> Result<OverlayStatus, String> {
    open(&app, "manual")
}

#[tauri::command]
pub fn close_overlay(app: AppHandle) -> Result<OverlayStatus, String> {
    close(&app, "manual")
}

#[tauri::command]
pub fn overlay_game_state(app: AppHandle) -> Option<OverlayGameState> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    game_state(&state, &settings)
}

#[tauri::command]
pub fn set_overlay_game_disabled(
    app: AppHandle,
    game_id: i64,
    disabled: bool,
) -> Result<OverlayStatus, String> {
    set_game_disabled(&app, game_id, disabled)
}

#[tauri::command]
pub fn overlay_show_big_picture(app: AppHandle) -> Result<(), String> {
    show_big_picture(&app)
}

/// Map the persisted trigger onto the detector's platform-free mirror.
pub fn trigger_choice(trigger: OverlayTrigger) -> detector::TriggerChoice {
    match trigger {
        OverlayTrigger::DoubleHome => detector::TriggerChoice::DoubleHome,
        OverlayTrigger::Chord => detector::TriggerChoice::Chord,
        OverlayTrigger::Keyboard => detector::TriggerChoice::Keyboard,
    }
}

/* ---------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::detector::{DetectorConfig, PadButton, PadEvent, TriggerChoice};

    #[test]
    fn newest_started_picks_the_last_launch() {
        assert_eq!(newest_started([].into_iter()), None);
        assert_eq!(newest_started([10].into_iter()), Some(0));
        assert_eq!(newest_started([10, 30, 20].into_iter()), Some(1));
        // Equal timestamps keep the first-seen entry (deterministic).
        assert_eq!(newest_started([10, 10].into_iter()), Some(0));
    }

    #[test]
    fn a_game_is_disabled_only_when_listed() {
        assert!(is_game_disabled(&[7, 42], 42));
        assert!(!is_game_disabled(&[7, 42], 43));
        assert!(!is_game_disabled(&[], 1));
    }

    #[test]
    fn the_presentation_roundtrips_as_camel_case() {
        for (value, expected) in [
            (OverlayPresentation::Closed, "closed"),
            (OverlayPresentation::Window, "window"),
            (OverlayPresentation::InApp, "inApp"),
        ] {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
    }

    #[test]
    fn listener_status_defaults_to_unavailable() {
        let status = OverlayListenerStatus::default();
        assert!(!status.available);
        assert!(status.controllers.is_empty());
    }

    #[test]
    fn trigger_choices_enable_the_expected_gestures() {
        let double = DetectorConfig::for_trigger(TriggerChoice::DoubleHome);
        assert!(double.guide_enabled);
        assert!(double.chord_enabled);

        let chord = DetectorConfig::for_trigger(TriggerChoice::Chord);
        assert!(!chord.guide_enabled);
        assert!(chord.chord_enabled);

        let keyboard = DetectorConfig::for_trigger(TriggerChoice::Keyboard);
        assert!(!keyboard.guide_enabled);
        assert!(!keyboard.chord_enabled);
    }

    /// The settings trigger and the detector's mirror must not drift apart.
    #[test]
    fn the_settings_trigger_maps_to_the_detector_trigger() {
        for (settings, expected) in [
            (OverlayTrigger::DoubleHome, TriggerChoice::DoubleHome),
            (OverlayTrigger::Chord, TriggerChoice::Chord),
            (OverlayTrigger::Keyboard, TriggerChoice::Keyboard),
        ] {
            assert_eq!(trigger_choice(settings), expected);
        }
    }

    #[test]
    fn a_patch_only_touches_the_fields_it_names() {
        let mut settings = LocalSettings {
            overlay_enabled: true,
            overlay_opacity: 0.8,
            overlay_tabs: vec!["game".into(), "library".into()],
            ..LocalSettings::default()
        };

        apply_patch(
            &mut settings,
            OverlaySettingsPatch {
                opacity: Some(0.5),
                ..OverlaySettingsPatch::default()
            },
        );
        assert_eq!(settings.overlay_opacity, 0.5);
        assert!(settings.overlay_enabled);
        assert_eq!(settings.overlay_tabs, vec!["game", "library"]);

        apply_patch(
            &mut settings,
            OverlaySettingsPatch {
                enabled: Some(false),
                trigger: Some(OverlayTrigger::Chord),
                tabs: Some(vec!["downloads".into()]),
                disabled_games: Some(vec![12]),
                ..OverlaySettingsPatch::default()
            },
        );
        assert!(!settings.overlay_enabled);
        assert_eq!(settings.overlay_trigger, OverlayTrigger::Chord);
        assert_eq!(settings.overlay_tabs, vec!["downloads"]);
        assert_eq!(settings.overlay_disabled_games, vec![12]);
    }

    #[test]
    fn closing_returns_focus_to_the_captured_window_or_never_steals_it() {
        // The game window was captured: restore it.
        assert_eq!(focus_return_plan(true, true), FocusReturn::Captured);
        assert_eq!(focus_return_plan(true, false), FocusReturn::Captured);
        // Nothing captured and a game is running: change nothing.
        assert_eq!(focus_return_plan(false, true), FocusReturn::Leave);
        // Nothing captured, no game: pull Ludex forward.
        assert_eq!(focus_return_plan(false, false), FocusReturn::MainWindow);
    }

    #[test]
    fn the_platform_note_mentions_the_fullscreen_limit() {
        assert!(platform_note().contains("fullscreen"));
        assert!(platform_note().contains("injection"));
    }

    fn healthy_input() -> OverlayDiagnosisInput {
        OverlayDiagnosisInput {
            enabled: true,
            game_disabled: false,
            has_game: true,
            trigger: OverlayTrigger::Chord,
            platform: OverlayPlatform::Windows,
            listener_available: true,
            listener_running: true,
            listener_reason: None,
            controller_count: 1,
            floating_window: true,
            fullscreen: OverlayFullscreen::Windowed,
        }
    }

    #[test]
    fn the_trigger_resolution_reports_what_can_actually_fire() {
        // Windows never delivers the Guide button: doubleHome falls back to
        // the chord instead of silently doing nothing.
        let windows = trigger_support(
            OverlayTrigger::DoubleHome,
            OverlayPlatform::Windows,
            true,
            1,
        );
        assert!(windows.deliverable);
        assert!(windows.guide_enabled);
        assert!(windows.chord_enabled);
        assert_eq!(windows.effective, "chord");
        assert_eq!(
            windows.note_code.as_deref(),
            Some("guide-unsupported-windows")
        );

        // Linux evdev exposes Guide: the configured gesture fires as-is.
        let linux = trigger_support(OverlayTrigger::DoubleHome, OverlayPlatform::Linux, true, 1);
        assert_eq!(linux.effective, "guide");
        assert!(linux.note_code.is_none());

        // The chord trigger never claims the Guide gesture.
        let chord = trigger_support(OverlayTrigger::Chord, OverlayPlatform::Windows, true, 2);
        assert_eq!(chord.effective, "chord");
        assert!(chord.deliverable);
        assert!(chord.note_code.is_none());

        // Keyboard-only arms no controller gesture at all.
        let keyboard = trigger_support(OverlayTrigger::Keyboard, OverlayPlatform::Windows, true, 1);
        assert!(!keyboard.deliverable);
        assert_eq!(keyboard.effective, "none");
        assert_eq!(keyboard.note_code.as_deref(), Some("trigger-keyboard"));

        // No pad, no listener, unsupported platform.
        let no_pad = trigger_support(OverlayTrigger::Chord, OverlayPlatform::Linux, true, 0);
        assert!(!no_pad.deliverable);
        assert_eq!(no_pad.note_code.as_deref(), Some("no-controller"));
        let no_listener =
            trigger_support(OverlayTrigger::Chord, OverlayPlatform::Windows, false, 1);
        assert!(!no_listener.deliverable);
        assert_eq!(
            no_listener.note_code.as_deref(),
            Some("listener-unavailable")
        );
        let mac = trigger_support(OverlayTrigger::Chord, OverlayPlatform::MacOs, false, 1);
        assert!(!mac.deliverable);
        assert_eq!(mac.note_code.as_deref(), Some("listener-unsupported"));
    }

    #[test]
    fn the_capability_matrix_matches_the_platform() {
        assert!(OverlayPlatform::Windows.supports_floating_window());
        assert!(OverlayPlatform::Linux.supports_floating_window());
        assert!(!OverlayPlatform::MacOs.supports_floating_window());

        assert!(OverlayPlatform::Windows.supports_global_listener());
        assert!(OverlayPlatform::Linux.supports_global_listener());
        assert!(!OverlayPlatform::MacOs.supports_global_listener());

        // XInput/WGI does not expose Guide; evdev does.
        assert!(!OverlayPlatform::Windows.supports_guide_button());
        assert!(OverlayPlatform::Linux.supports_guide_button());

        let current = capabilities();
        assert_eq!(current.platform, OverlayPlatform::current().as_str());
        assert_eq!(
            current.global_listener,
            cfg!(any(target_os = "windows", target_os = "linux"))
        );
    }

    #[test]
    fn a_healthy_setup_reports_ready() {
        let diagnosis = diagnose(&healthy_input());
        assert_eq!(diagnosis.summary, "ready");
        assert!(diagnosis.can_open);
        assert!(diagnosis.trigger_deliverable);
        assert!(diagnosis.over_game);
        assert!(diagnosis
            .checks
            .iter()
            .all(|check| check.level != CHECK_BLOCKING && check.level != CHECK_WARNING));
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "trigger-ready"));
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "game-windowed"));
    }

    #[test]
    fn every_refusal_has_its_own_check() {
        // Globally disabled.
        let disabled = OverlayDiagnosisInput {
            enabled: false,
            ..healthy_input()
        };
        let diagnosis = diagnose(&disabled);
        assert_eq!(diagnosis.summary, "overlay-disabled");
        assert!(!diagnosis.can_open);
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "overlay-disabled" && check.level == CHECK_BLOCKING));

        // Per-game opt-out.
        let game_disabled = OverlayDiagnosisInput {
            game_disabled: true,
            ..healthy_input()
        };
        let diagnosis = diagnose(&game_disabled);
        assert_eq!(diagnosis.summary, "game-disabled");
        assert!(!diagnosis.can_open);

        // Listener failed: reasoning text is carried through.
        let listener_failed = OverlayDiagnosisInput {
            listener_available: false,
            listener_reason: Some("Controllers cannot be read: permission denied".into()),
            ..healthy_input()
        };
        let diagnosis = diagnose(&listener_failed);
        assert_eq!(diagnosis.summary, "listener-failed");
        assert!(diagnosis.checks.iter().any(|check| {
            check.id == "listener-failed" && check.message.contains("permission denied")
        }));
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "trigger-unavailable"));

        // Listener thread dead.
        let listener_stopped = OverlayDiagnosisInput {
            listener_running: false,
            ..healthy_input()
        };
        let diagnosis = diagnose(&listener_stopped);
        assert_eq!(diagnosis.summary, "listener-stopped");

        // No controller.
        let no_controller = OverlayDiagnosisInput {
            controller_count: 0,
            ..healthy_input()
        };
        let diagnosis = diagnose(&no_controller);
        assert_eq!(diagnosis.summary, "no-controller");
        assert!(!diagnosis.trigger_deliverable);

        // Guide unsupported on Windows: a warning, but the chord still fires.
        let guide = OverlayDiagnosisInput {
            trigger: OverlayTrigger::DoubleHome,
            ..healthy_input()
        };
        let diagnosis = diagnose(&guide);
        assert_eq!(diagnosis.summary, "guide-unsupported-windows");
        assert!(diagnosis.trigger_deliverable);
        assert_eq!(
            trigger_support(
                OverlayTrigger::DoubleHome,
                OverlayPlatform::Windows,
                true,
                1
            )
            .effective,
            "chord"
        );

        // Keyboard-only: no blocking row, but no pad gesture either.
        let keyboard = OverlayDiagnosisInput {
            trigger: OverlayTrigger::Keyboard,
            ..healthy_input()
        };
        let diagnosis = diagnose(&keyboard);
        assert_eq!(diagnosis.summary, "ready");
        assert!(!diagnosis.trigger_deliverable);
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "trigger-keyboard"));

        // Exclusive-fullscreen risk.
        let fullscreen = OverlayDiagnosisInput {
            fullscreen: OverlayFullscreen::Fullscreen,
            ..healthy_input()
        };
        let diagnosis = diagnose(&fullscreen);
        assert_eq!(diagnosis.summary, "game-fullscreen");
        assert!(!diagnosis.over_game);

        // No floating window (macOS).
        let no_window = OverlayDiagnosisInput {
            platform: OverlayPlatform::MacOs,
            listener_available: false,
            floating_window: false,
            ..healthy_input()
        };
        let diagnosis = diagnose(&no_window);
        // The missing listener is the first warning, then the missing window.
        assert_eq!(diagnosis.summary, "trigger-unavailable");
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "listener-unsupported"));
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "no-floating-window"));
    }

    #[test]
    fn no_game_is_an_informational_state() {
        let diagnosis = diagnose(&OverlayDiagnosisInput {
            has_game: false,
            ..healthy_input()
        });
        assert_eq!(diagnosis.summary, "ready");
        assert!(diagnosis.can_open);
        assert!(diagnosis
            .checks
            .iter()
            .any(|check| check.id == "no-game" && check.level == CHECK_INFO));
    }

    #[test]
    fn the_trigger_hint_points_at_the_manual_fallback() {
        let status = trigger_support(
            OverlayTrigger::DoubleHome,
            OverlayPlatform::Windows,
            true,
            1,
        );
        let note = status.note.unwrap_or_default();
        assert!(note.contains("LB+RB+Start"));

        let keyboard = trigger_support(OverlayTrigger::Keyboard, OverlayPlatform::Windows, true, 1);
        assert!(!keyboard.guide_enabled);
        assert!(!keyboard.chord_enabled);
    }

    #[test]
    fn a_trigger_event_serializes_the_last_attempt() {
        let event = OverlayTriggerEvent {
            at: "2026-01-01T00:00:00Z".to_string(),
            source: "controller".to_string(),
            action: "open".to_string(),
            outcome: "ignored".to_string(),
            detail: Some("The overlay is turned off.".to_string()),
        };
        let json = serde_json::to_value(event).unwrap();
        assert_eq!(json["source"], "controller");
        assert_eq!(json["outcome"], "ignored");
        assert_eq!(json["detail"], "The overlay is turned off.");
    }

    #[test]
    fn the_runtime_records_the_last_trigger_event() {
        let runtime = OverlayRuntime::default();
        assert!(runtime.last_trigger().is_none());
        let event = OverlayTriggerEvent {
            at: "2026-01-01T00:00:00Z".to_string(),
            source: "manual".to_string(),
            action: "open".to_string(),
            outcome: "openedWindow".to_string(),
            detail: None,
        };
        runtime.record_trigger(event.clone());
        assert_eq!(runtime.last_trigger(), Some(event));
        // The listener flag is independent from the recorded event.
        assert!(!runtime.listener_running());
        runtime.set_listener_running(true);
        assert!(runtime.listener_running());
    }

    // Keep the detector dependency used from this module's tests, so a refactor
    // that drops the re-export fails here instead of only in global_listener.
    #[test]
    fn the_detector_is_reachable_through_the_overlay_module() {
        let mut detector =
            detector::GestureDetector::new(DetectorConfig::for_trigger(TriggerChoice::Chord));
        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 2);
        assert!(detector
            .handle(PadEvent::Pressed(PadButton::Start), 3)
            .is_some());
    }
}
