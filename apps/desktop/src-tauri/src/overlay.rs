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
    pub available: bool,
    pub platform: String,
    pub reason: Option<String>,
    pub controllers: Vec<OverlayController>,
}

/// The running game shown in the overlay.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayGameState {
    pub session_id: String,
    pub game_id: i64,
    pub title: String,
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
    presentation: Mutex<OverlayPresentation>,
    listener: Mutex<OverlayListenerStatus>,
    focus_return: Mutex<Option<FocusToken>>,
}

impl Default for OverlayRuntime {
    fn default() -> Self {
        Self {
            open: AtomicBool::new(false),
            presentation: Mutex::new(OverlayPresentation::Closed),
            listener: Mutex::new(OverlayListenerStatus {
                backend: "none".to_string(),
                available: false,
                platform: std::env::consts::OS.to_string(),
                reason: Some("The native gamepad listener has not started.".to_string()),
                controllers: Vec::new(),
            }),
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
        self.listener
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    pub fn set_listener_status(&self, status: OverlayListenerStatus) {
        if let Ok(mut current) = self.listener.lock() {
            *current = status;
        }
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
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    global_listener::start(app.clone());

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let state = app.state::<AppState>();
        state.overlay.set_listener_status(OverlayListenerStatus {
            backend: "unsupported".to_string(),
            available: false,
            platform: std::env::consts::OS.to_string(),
            reason: Some(
                "The global controller listener is only available on Windows and Linux. Use Shift+Tab while Ludex is focused."
                    .to_string(),
            ),
            controllers: Vec::new(),
        });
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
    started_at: String,
    elapsed_seconds: i64,
    /// Playtime of finished sessions recorded before this launch.
    finished_seconds: i64,
}

/// Read the game currently running (the newest launch when several are).
fn running_game(state: &AppState) -> Option<RunningGame> {
    // Snapshot the process list under its lock, then release it before touching
    // the database: no nested locks between the process map and SQLite.
    let (session_id, game_id, started_at, elapsed_seconds) = {
        let processes = state.processes.lock().ok()?;
        let (session_id, entry) = processes.iter().max_by_key(|(_, entry)| entry.started_at)?;
        (
            session_id.clone(),
            entry.game_id,
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
    OverlayStatus {
        enabled: settings.overlay_enabled,
        open: state.overlay.is_open(),
        trigger: settings.overlay_trigger,
        opacity: settings.overlay_opacity,
        scale: settings.overlay_scale,
        tabs: settings.overlay_tabs.clone(),
        disabled_games: settings.overlay_disabled_games.clone(),
        game_disabled: game.as_ref().map(|game| game.disabled).unwrap_or(false),
        game,
        presentation: state.overlay.presentation(),
        listener: state.overlay.listener_status(),
        platform_note: platform_note().to_string(),
        keyboard_shortcut: KEYBOARD_SHORTCUT.to_string(),
    }
}

/* ------------------------------ open / close ------------------------------ */

/// Open the overlay (idempotent). Refused when the feature or the game is off.
pub fn open(app: &AppHandle) -> Result<OverlayStatus, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    if !settings.overlay_enabled {
        return Err(
            "The in-game overlay is turned off. Enable it in Settings → Profiles.".to_string(),
        );
    }
    if let Some(game) = running_game(&state) {
        if is_game_disabled(&settings.overlay_disabled_games, game.game_id) {
            return Err("The in-game overlay is turned off for this game.".to_string());
        }
    }

    // Idempotent: a second toggle while the overlay is already open must not
    // turn a floating window into an in-app panel (or the reverse).
    if state.overlay.is_open() {
        if state.overlay.presentation() == OverlayPresentation::Window {
            if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        return Ok(status_for(app, &state));
    }

    // Capture the foreground window before the overlay steals focus, so closing
    // it hands input back to the game (Windows: exact HWND; X11: xdotool).
    state.overlay.store_focus_token(capture_foreground(app));
    state.overlay.set_open(true);

    // Phase 1: while Ludex itself is focused, draw the overlay inside the main
    // window. Phase 2: a game owns the focus, so a second always-on-top window
    // is required.
    let main_focused = app
        .get_webview_window("main")
        .map(|window| window.is_focused().unwrap_or(false))
        .unwrap_or(false);
    if main_focused {
        state.overlay.set_presentation(OverlayPresentation::InApp);
        let _ = app.emit_to("main", "overlay://open", ());
        let status = status_for(app, &state);
        broadcast(app, &status);
        let _ = app.emit_to("overlay", "overlay://settings", &status);
        return Ok(status);
    }

    match show_overlay_window(app) {
        Ok(()) => state.overlay.set_presentation(OverlayPresentation::Window),
        Err(error) => {
            tracing::warn!(error = %error, "the floating overlay window is unavailable; using the in-app overlay");
            state.overlay.set_presentation(OverlayPresentation::InApp);
            let _ = app.emit_to("main", "overlay://open", ());
        }
    }

    let status = status_for(app, &state);
    broadcast(app, &status);
    let _ = app.emit_to("overlay", "overlay://settings", &status);
    Ok(status)
}

/// Close the overlay and restore focus. Never touches the game process.
pub fn close(app: &AppHandle) -> Result<OverlayStatus, String> {
    let state = app.state::<AppState>();
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = window.hide();
    }
    state.overlay.set_open(false);
    state.overlay.set_presentation(OverlayPresentation::Closed);
    let _ = app.emit_to("main", "overlay://close", ());
    restore_foreground(app, &state);
    let status = status_for(app, &state);
    broadcast(app, &status);
    Ok(status)
}

/// Listener entry point: open when closed, close when open.
pub fn toggle(app: &AppHandle) {
    let state = app.state::<AppState>();
    let result = if state.overlay.is_open() {
        close(app)
    } else {
        open(app)
    };
    if let Err(error) = result {
        // A disabled overlay must not spam the log on every Home press.
        tracing::debug!(error = %error, "overlay toggle ignored");
    }
}

fn show_overlay_window(app: &AppHandle) -> Result<(), String> {
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
            builder
                .build()
                .map_err(|error| format!("Could not create the overlay window: {error}"))?
        }
    };

    position_on_current_monitor(app, &window);
    let _ = window.set_always_on_top(true);
    let _ = window.set_skip_taskbar(true);
    window
        .show()
        .map_err(|error| format!("Could not show the overlay window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("Could not focus the overlay window: {error}"))?;

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
        status = close(app)?;
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
        status = close(app)?;
    }
    let _ = app.emit_to("overlay", "overlay://settings", &status);
    Ok(status)
}

/* ------------------------------- big picture ------------------------------ */

/// Bring the main window forward on Big Picture and close the overlay.
pub fn show_big_picture(app: &AppHandle) -> Result<(), String> {
    let _ = close(app);
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
    let output = std::process::Command::new("xdotool")
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
        let restored = std::process::Command::new("xdotool")
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

/* -------------------------------- commands ------------------------------- */

#[tauri::command]
pub fn overlay_status(app: AppHandle, state: tauri::State<'_, AppState>) -> OverlayStatus {
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
    open(&app)
}

#[tauri::command]
pub fn close_overlay(app: AppHandle) -> Result<OverlayStatus, String> {
    close(&app)
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
