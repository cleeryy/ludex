//! Resume a running game and report its live session (#191).
//!
//! The game page needs more than the overlay's newest-game snapshot
//! (`overlay_game_state`): it asks for one game and needs the root PID and the
//! executable. The running session therefore comes from the same process
//! registry the launch watcher maintains, so a session exists exactly while a
//! tracked process tree is alive. The overlay module keeps its own shape
//! (#144/#190); nothing here duplicates its focus-return token.
//!
//! Resuming never starts a second instance: it finds a top-level window owned
//! by the game PID and brings it back. When the platform cannot find or focus
//! one (a launcher spawned the real game, an exclusive-fullscreen window, a
//! Wayland compositor that refuses focus), Ludex steps out of the way and asks
//! the user to Alt+Tab back.
//!
//! Platform limits are documented in `docs/CONTRACT.md`: Windows uses
//! `EnumWindows`/`SetForegroundWindow`, Linux shells out to `xdotool` on X11,
//! and macOS has no window-by-PID lookup here, so it always takes the Alt+Tab
//! fallback.

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::db;
use crate::state::AppState;

/// The live session of one game, as the game page needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSession {
    pub session_id: String,
    pub game_id: i64,
    /// Root PID of the launched process tree.
    pub pid: u32,
    /// RFC 3339 launch time, seconds precision.
    pub started_at: String,
    /// Seconds since the launch, as of this call.
    pub elapsed_seconds: i64,
    /// Executable stored for the install (relative to the install folder).
    pub executable: Option<String>,
    pub title: String,
}

/// The fields session resolution needs, free of the child-process handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub game_id: i64,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub elapsed_seconds: i64,
}

/// The running session for `game_id`: the newest launch when a game somehow
/// has two entries. Sessions of other games are ignored, so the page never
/// shows another game as "running".
pub fn pick_session(
    game_id: i64,
    sessions: impl IntoIterator<Item = SessionSnapshot>,
) -> Option<SessionSnapshot> {
    sessions
        .into_iter()
        .filter(|session| session.game_id == game_id)
        .max_by_key(|session| session.started_at)
}

/// Resolve the live session for one game from the process registry.
pub fn running_game_session(state: &AppState, game_id: i64) -> Option<GameSession> {
    let snapshot = {
        let processes = state.processes.lock().ok()?;
        pick_session(
            game_id,
            processes.iter().map(|(session_id, entry)| SessionSnapshot {
                session_id: session_id.clone(),
                game_id: entry.game_id,
                pid: entry.pid,
                started_at: entry.started_at,
                elapsed_seconds: i64::try_from(entry.started.elapsed().as_secs())
                    .unwrap_or(i64::MAX),
            }),
        )?
    };

    // Lock order: the process map is already released; the index read comes
    // second, as in the overlay and the launcher.
    let (title, executable) = state
        .db
        .lock()
        .ok()
        .and_then(|connection| db::get_installed(&connection, game_id).ok().flatten())
        .map(|installed| (installed.title, installed.executable))
        .unwrap_or_else(|| (format!("Game {game_id}"), None));

    Some(GameSession {
        session_id: snapshot.session_id,
        game_id,
        pid: snapshot.pid,
        started_at: snapshot
            .started_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        elapsed_seconds: snapshot.elapsed_seconds,
        executable,
        title,
    })
}

/* ----------------------------- window lookup ----------------------------- */

/// A top-level window owned by the game's PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCandidate {
    /// Platform handle: `HWND` (decimal) on Windows, xdotool id on X11.
    pub id: String,
    /// Mapped and not hidden.
    pub visible: bool,
    /// Iconified/minimised; restoring it is enough to bring it back.
    pub minimized: bool,
}

/// Which window to bring back: a visible, non-minimised one first, then a
/// minimised one (restore), then a hidden one (show). `None` means the game has
/// no top-level window yet (a launcher owns it, or it is still starting).
pub fn choose_window(candidates: &[WindowCandidate]) -> Option<&WindowCandidate> {
    candidates
        .iter()
        .find(|window| window.visible && !window.minimized)
        .or_else(|| candidates.iter().find(|window| window.visible))
        .or_else(|| candidates.first())
}

/// Result of focusing the game window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The window was found and is now in the foreground.
    Focused { window: String },
    /// No top-level window for the PID: the Alt+Tab fallback applies.
    NoWindow,
    /// The window exists but the window manager refused the focus request.
    Refused { window: String },
}

/// The lookup decision tree, with the platform primitives injected so the
/// ordering and the fallback are unit-tested without a live window.
pub fn resume_with(
    find: impl FnOnce() -> Vec<WindowCandidate>,
    focus: impl Fn(&str) -> bool,
) -> ResumeOutcome {
    let candidates = find();
    let Some(window) = choose_window(&candidates) else {
        return ResumeOutcome::NoWindow;
    };
    if focus(&window.id) {
        ResumeOutcome::Focused {
            window: window.id.clone(),
        }
    } else {
        ResumeOutcome::Refused {
            window: window.id.clone(),
        }
    }
}

/// What the game page is told after a resume attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeResult {
    /// True when the game window is now focused.
    pub focused: bool,
    /// Platform window handle that was chosen, when any.
    pub window: Option<String>,
    /// `noWindow` / `focusRefused` when the Alt+Tab fallback was used.
    pub reason: Option<String>,
}

/* ------------------------------- platform -------------------------------- */

#[cfg(windows)]
fn lookup_and_focus(pid: u32) -> ResumeOutcome {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    struct Search {
        pid: u32,
        windows: Vec<WindowCandidate>,
    }

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Search` pointer handed to `EnumWindows` and
        // the callback only runs during that call.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: `hwnd` comes from the system enumerator.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == search.pid {
            search.windows.push(WindowCandidate {
                id: (hwnd.0 as isize).to_string(),
                // SAFETY: the window handle is live for the duration of the call.
                visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
                minimized: unsafe { IsIconic(hwnd) }.as_bool(),
            });
        }
        BOOL(1)
    }

    let mut search = Search {
        pid,
        windows: Vec::new(),
    };
    // SAFETY: the pointer stays valid until `EnumWindows` returns.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut search as *mut Search as isize)) };
    let windows = std::mem::take(&mut search.windows);
    resume_with(|| windows, focus_window)
}

#[cfg(windows)]
fn focus_window(id: &str) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    let Ok(handle) = id.parse::<isize>() else {
        return false;
    };
    let hwnd = HWND(handle as *mut std::ffi::c_void);
    // SAFETY: the handle was enumerated for this PID moments ago. A window that
    // closed in between simply fails both calls; nothing is dereferenced here.
    unsafe {
        // A minimised game is restored; a hidden one is shown. `windowactivate`
        // is not enough on Windows: `ShowWindow` also covers a game that hid
        // its own window on focus loss.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        // Best effort: Windows may refuse foreground changes requested by a
        // background process, and the caller then asks for Alt+Tab.
        SetForegroundWindow(hwnd).as_bool()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn lookup_and_focus(pid: u32) -> ResumeOutcome {
    let pid = pid.to_string();
    let Some(all) = xdotool(&["search", "--pid", &pid]) else {
        tracing::debug!("xdotool could not list the game windows");
        return ResumeOutcome::NoWindow;
    };
    let mapped: Vec<String> = xdotool(&["search", "--onlyvisible", "--pid", &pid])
        .map(|output| output.lines().map(str::trim).map(str::to_string).collect())
        .unwrap_or_default();
    let windows: Vec<WindowCandidate> = all
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|id| WindowCandidate {
            id: id.to_string(),
            visible: mapped.iter().any(|mapped_id| mapped_id.as_str() == id),
            // An unmapped X11 window is either iconified or hidden; `windowmap`
            // restores both, so it is treated as minimised.
            minimized: !mapped.iter().any(|mapped_id| mapped_id.as_str() == id),
        })
        .collect();
    resume_with(|| windows, focus_window)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn focus_window(id: &str) -> bool {
    // `windowactivate` alone does not restore an iconified window.
    let _ = xdotool(&["windowmap", id]);
    xdotool(&["windowactivate", "--sync", id]).is_some()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdotool(args: &[&str]) -> Option<String> {
    let output = crate::process::capture("xdotool")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
fn lookup_and_focus(_pid: u32) -> ResumeOutcome {
    // No window-by-PID lookup here: AppKit only exposes the applications this
    // process owns. The Alt+Tab fallback is documented in the contract.
    tracing::debug!("window lookup by PID is not available on macOS");
    ResumeOutcome::NoWindow
}

/* -------------------------------- commands ------------------------------- */

/// The running session of one game, or `null`. Polled by the game page while
/// it is open; `game://exited` is the event that invalidates it.
#[tauri::command]
pub fn game_session(app: AppHandle, game_id: i64) -> Option<GameSession> {
    let state = app.state::<AppState>();
    running_game_session(&state, game_id)
}

/// True while the launch watcher still tracks this session.
fn session_alive(state: &AppState, session_id: &str) -> bool {
    state
        .processes
        .lock()
        .map(|processes| processes.contains_key(session_id))
        .unwrap_or(false)
}

/// Bring the running game's window back. Never launches a second instance.
pub fn resume(app: &AppHandle, game_id: i64) -> Result<ResumeResult, String> {
    let state = app.state::<AppState>();
    let session = running_game_session(&state, game_id)
        .ok_or_else(|| "This game is not running. Press Play to start it.".to_string())?;

    let outcome = lookup_and_focus(session.pid);
    // The game may have exited during the lookup: hiding Ludex and asking for
    // Alt+Tab would then point at nothing.
    if !session_alive(&state, &session.session_id) {
        return Err("The game has already exited.".to_string());
    }
    match outcome {
        ResumeOutcome::Focused { window } => {
            tracing::info!(
                session_id = %session.session_id,
                pid = session.pid,
                window = %window,
                "resumed the game window"
            );
            Ok(ResumeResult {
                focused: true,
                window: Some(window),
                reason: None,
            })
        }
        ResumeOutcome::NoWindow => {
            tracing::warn!(
                session_id = %session.session_id,
                pid = session.pid,
                game_id,
                "no top-level window found for the game; hiding Ludex for Alt+Tab"
            );
            crate::tray::hide_window(app);
            Ok(ResumeResult {
                focused: false,
                window: None,
                reason: Some("noWindow".to_string()),
            })
        }
        ResumeOutcome::Refused { window } => {
            tracing::warn!(
                session_id = %session.session_id,
                pid = session.pid,
                window = %window,
                game_id,
                "the window manager refused to focus the game; hiding Ludex for Alt+Tab"
            );
            crate::tray::hide_window(app);
            Ok(ResumeResult {
                focused: false,
                window: Some(window),
                reason: Some("focusRefused".to_string()),
            })
        }
    }
}

/// Resume the game, with the platform lookup run off the main thread.
#[tauri::command]
pub async fn resume_game(app: AppHandle, game_id: i64) -> Result<ResumeResult, String> {
    tauri::async_runtime::spawn_blocking(move || resume(&app, game_id))
        .await
        .map_err(|error| format!("The resume task failed: {error}"))?
}

/* ---------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, game_id: i64, started_at: DateTime<Utc>) -> SessionSnapshot {
        SessionSnapshot {
            session_id: id.to_string(),
            game_id,
            pid: 100,
            started_at,
            elapsed_seconds: 5,
        }
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn window(id: &str, visible: bool, minimized: bool) -> WindowCandidate {
        WindowCandidate {
            id: id.to_string(),
            visible,
            minimized,
        }
    }

    #[test]
    fn the_newest_session_of_that_game_wins() {
        let sessions = vec![
            session("old", 7, at(100)),
            session("other", 8, at(300)),
            session("new", 7, at(200)),
        ];
        let picked = pick_session(7, sessions).unwrap();
        assert_eq!(picked.session_id, "new");
        assert_eq!(picked.elapsed_seconds, 5);
        assert!(pick_session(9, Vec::new()).is_none());
    }

    #[test]
    fn a_single_session_is_resolved_regardless_of_age() {
        let sessions = vec![session("only", 3, at(10))];
        assert_eq!(pick_session(3, sessions).unwrap().session_id, "only");
    }

    #[test]
    fn the_visible_restored_window_is_preferred() {
        let candidates = vec![
            window("hidden", false, false),
            window("minimized", true, true),
            window("normal", true, false),
        ];
        assert_eq!(choose_window(&candidates).unwrap().id, "normal");
    }

    #[test]
    fn a_minimized_window_beats_a_hidden_one() {
        let candidates = vec![
            window("hidden", false, false),
            window("minimized", true, true),
        ];
        assert_eq!(choose_window(&candidates).unwrap().id, "minimized");
    }

    #[test]
    fn a_hidden_window_is_the_last_resort() {
        let candidates = vec![window("hidden", false, false)];
        assert_eq!(choose_window(&candidates).unwrap().id, "hidden");
        assert!(choose_window(&[]).is_none());
    }

    #[test]
    fn the_finder_result_is_focused() {
        let focused = std::cell::RefCell::new(Vec::new());
        let outcome = resume_with(
            || vec![window("42", true, false)],
            |id| {
                focused.borrow_mut().push(id.to_string());
                true
            },
        );
        assert_eq!(
            outcome,
            ResumeOutcome::Focused {
                window: "42".to_string()
            }
        );
        assert_eq!(focused.into_inner(), vec!["42".to_string()]);
    }

    #[test]
    fn no_window_falls_back_to_alt_tab() {
        let outcome = resume_with(Vec::new, |_| panic!("nothing to focus"));
        assert_eq!(outcome, ResumeOutcome::NoWindow);
    }

    #[test]
    fn a_refused_focus_is_reported_for_the_alt_tab_fallback() {
        let outcome = resume_with(|| vec![window("7", true, false)], |_| false);
        assert_eq!(
            outcome,
            ResumeOutcome::Refused {
                window: "7".to_string()
            }
        );
    }
}
