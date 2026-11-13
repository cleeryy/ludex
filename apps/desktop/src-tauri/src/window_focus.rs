//! Bringing a game's own top-level window to the foreground (#219).
//!
//! The overlay (#144, `overlay.rs`) captures the window that was focused when it
//! opened and hands focus back when it closes. This module is the other
//! direction: after Ludex starts a game, find the top-level window that game's
//! PID owns and make it the foreground window, so the player never has to
//! Alt+Tab into a game that is already running. The Resume action (#191,
//! `resume.rs`) uses the same lookup and the same focus ladder: one window
//! primitive per platform, never a second copy.
//!
//! - **Windows**: `EnumWindows` + `GetWindowThreadProcessId` list the PID's
//!   windows (tool windows are skipped), `ShowWindow` restores or shows the
//!   candidate, then the focus goes up a ladder: `SetForegroundWindow`, then the
//!   foreground-lock workaround (`AttachThreadInput` on the foreground thread,
//!   detached again immediately), then `BringWindowToTop`. Windows only lets the
//!   foreground process hand focus over, so `AllowSetForegroundWindow(game_pid)`
//!   is also called right after the spawn: the game may take the foreground
//!   itself while it starts.
//! - **Linux/X11**: `xdotool search --pid`, `windowmap`, `windowactivate
//!   --sync`. Wayland compositors decide focus themselves and may ignore the
//!   request; the failure is logged and reported, never fatal.
//! - **macOS**: there is no window-by-PID lookup (`AppKit` only exposes the
//!   application's own windows), so focus-on-launch is a documented no-op:
//!   [`focus_supported`] is false and the launcher leaves focus alone instead of
//!   waiting or showing a warning on every launch. `open -a` is deliberately not
//!   used: it activates an application bundle, and for a bare executable it can
//!   start a second instance.
//!
//! A focus failure never fails a launch. The launcher logs the outcome and tells
//! the shell through `game://focus`, which points at the Resume/Force close
//! actions that already exist (#191).

use std::time::{Duration, Instant};

/// A top-level window owned by a game's PID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCandidate {
    /// Platform handle: `HWND` (decimal) on Windows, xdotool id on X11.
    pub id: String,
    /// Mapped and not hidden.
    pub visible: bool,
    /// Iconified/minimised; restoring it is enough to bring it back.
    pub minimized: bool,
}

/// Which window to bring forward: a visible, non-minimised one first, then a
/// minimised one (restore), then a hidden one (show). `None` means the game has
/// no top-level window yet (a launcher owns it, or it is still starting).
pub fn choose_window(candidates: &[WindowCandidate]) -> Option<&WindowCandidate> {
    candidates
        .iter()
        .find(|window| window.visible && !window.minimized)
        .or_else(|| candidates.iter().find(|window| window.visible))
        .or_else(|| candidates.first())
}

/// How one focus request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusOutcome {
    /// The window was found and is now in the foreground.
    Focused { window: String },
    /// No top-level window for the PID: the Alt+Tab fallback applies.
    NoWindow,
    /// The window exists but the window manager refused the focus request.
    Refused { window: String },
}

/// One rung of the Windows foreground ladder (#219), in the order it is tried.
///
/// Only Windows runs the ladder (Linux and macOS have a single request); it is
/// compiled for tests everywhere so the ordering is covered on CI's Linux and
/// macOS runners too.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusStep {
    /// Plain `SetForegroundWindow`.
    Request,
    /// The window manager refused: attach to the foreground thread first.
    ForegroundLock,
    /// Last resort: `BringWindowToTop`, then ask again.
    BringToTop,
}

/// The focus ladder, with the platform call injected so the ordering is tested
/// without a live window: the first rung that reports success wins and no later
/// rung runs. `None` means every rung was refused.
#[cfg(any(windows, test))]
pub fn focus_ladder(mut attempt: impl FnMut(FocusStep) -> bool) -> Option<FocusStep> {
    [
        FocusStep::Request,
        FocusStep::ForegroundLock,
        FocusStep::BringToTop,
    ]
    .into_iter()
    .find(|step| attempt(*step))
}

/// The lookup decision tree, with the platform primitives injected so the
/// ordering and the fallback are unit-tested without a live window.
pub fn focus_with(
    find: impl FnOnce() -> Vec<WindowCandidate>,
    focus: impl Fn(&str) -> bool,
) -> FocusOutcome {
    let candidates = find();
    let Some(window) = choose_window(&candidates) else {
        return FocusOutcome::NoWindow;
    };
    if focus(&window.id) {
        FocusOutcome::Focused {
            window: window.id.clone(),
        }
    } else {
        FocusOutcome::Refused {
            window: window.id.clone(),
        }
    }
}

/// How many poll attempts fit in `budget`, always at least one.
///
/// The game's first window (a splash screen, a launcher handing over, a slow
/// DirectX init) can take several seconds; polling every
/// [`LAUNCH_FOCUS_INTERVAL`] for [`LAUNCH_FOCUS_BUDGET`] gives 120 attempts.
pub fn focus_attempts(budget: Duration, interval: Duration) -> u32 {
    if interval.is_zero() {
        return 1;
    }
    let attempts = budget.as_millis() / interval.as_millis();
    u32::try_from(attempts).unwrap_or(u32::MAX).max(1)
}

/// Wait for the game's window, bounded by the attempt budget.
///
/// `attempt` is retried only while there is *no* window: a found window is
/// final, and a refused focus does not get a second chance (the platform ladder
/// already tried everything). `wait` is injected so a test never sleeps.
pub fn wait_for_window(
    max_attempts: u32,
    mut attempt: impl FnMut() -> FocusOutcome,
    mut wait: impl FnMut(),
) -> FocusOutcome {
    let attempts = max_attempts.max(1);
    for index in 0..attempts {
        let outcome = attempt();
        if outcome != FocusOutcome::NoWindow {
            return outcome;
        }
        if index + 1 < attempts {
            wait();
        }
    }
    FocusOutcome::NoWindow
}

/// The launch waits this long for the game's first top-level window.
pub const LAUNCH_FOCUS_BUDGET: Duration = Duration::from_secs(30);

/// Poll interval while waiting for the game's first top-level window.
pub const LAUNCH_FOCUS_INTERVAL: Duration = Duration::from_millis(250);

/// True when this platform can look up a window by PID and focus it.
///
/// macOS cannot (see the module docs), so the launcher leaves focus alone there
/// instead of collecting a guaranteed failure and warning the player on every
/// launch.
pub fn focus_supported() -> bool {
    cfg!(not(target_os = "macos"))
}

/// Wait for the game's window and focus it. Blocking: call from a worker thread,
/// never from the launch command or the UI thread.
pub fn wait_and_focus(pid: u32) -> (FocusOutcome, Duration) {
    let started = Instant::now();
    let outcome = wait_for_window(
        focus_attempts(LAUNCH_FOCUS_BUDGET, LAUNCH_FOCUS_INTERVAL),
        || lookup_and_focus(pid),
        || std::thread::sleep(LAUNCH_FOCUS_INTERVAL),
    );
    (outcome, started.elapsed())
}

/* ------------------------------- platform -------------------------------- */

#[cfg(windows)]
pub fn grant_foreground(pid: u32) {
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    // SAFETY: no handle is involved. Windows refuses the grant when Ludex is
    // not the foreground process, which only means the ladder below has to do
    // the work.
    match unsafe { AllowSetForegroundWindow(pid) } {
        Ok(()) => tracing::debug!(pid, "let the game take the foreground while it starts"),
        Err(error) => {
            tracing::debug!(pid, error = %error, "the game was not granted foreground rights")
        }
    }
}

/// No-op: only Windows has a per-process foreground grant.
#[cfg(not(windows))]
pub fn grant_foreground(pid: u32) {
    let _ = pid;
}

#[cfg(windows)]
fn lookup_candidates(pid: u32) -> Vec<WindowCandidate> {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowLongW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        GWL_EXSTYLE, WS_EX_TOOLWINDOW,
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
            // SAFETY: the window handle is live for the duration of the call.
            let tool =
                unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } & WS_EX_TOOLWINDOW.0 as i32 != 0;
            if !tool {
                search.windows.push(WindowCandidate {
                    id: (hwnd.0 as isize).to_string(),
                    visible: unsafe { IsWindowVisible(hwnd) }.as_bool(),
                    minimized: unsafe { IsIconic(hwnd) }.as_bool(),
                });
            }
        }
        BOOL(1)
    }

    let mut search = Search {
        pid,
        windows: Vec::new(),
    };
    // SAFETY: the pointer stays valid until `EnumWindows` returns.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut search as *mut Search as isize)) };
    search.windows
}

#[cfg(windows)]
fn focus_window(id: &str) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    let Ok(handle) = id.parse::<isize>() else {
        return false;
    };
    let hwnd = HWND(handle as *mut std::ffi::c_void);
    // SAFETY: the handle was enumerated for this PID moments ago. A window that
    // closed in between simply fails the calls; nothing is dereferenced here.
    unsafe {
        // A minimised game is restored; a hidden one is shown. `windowactivate`
        // is not enough on Windows: `ShowWindow` also covers a game that hid
        // its own window on focus loss.
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        } else {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
    let focused = focus_ladder(|step| {
        // SAFETY: same live handle; each call only asks Windows for the
        // foreground and reports whether it agreed.
        unsafe {
            match step {
                FocusStep::Request => SetForegroundWindow(hwnd).as_bool(),
                FocusStep::ForegroundLock => attach_and_focus(hwnd),
                FocusStep::BringToTop => {
                    let _ = BringWindowToTop(hwnd);
                    SetForegroundWindow(hwnd).as_bool()
                }
            }
        }
    });
    match focused {
        Some(step) => {
            tracing::debug!(window = %id, step = ?step, "the game window took the foreground");
            true
        }
        None => {
            tracing::debug!(window = %id, "the window manager refused the focus request");
            false
        }
    }
}

/// The Windows foreground-lock workaround: a process that does not own the
/// foreground may not change it, so attach to the foreground thread's input for
/// the duration of the request and detach immediately.
#[cfg(windows)]
fn attach_and_focus(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    // SAFETY: all calls take ids or handles owned by this process; the attach is
    // undone below even when the focus request fails.
    unsafe {
        let current = GetCurrentThreadId();
        let foreground = GetForegroundWindow();
        let owner = if foreground.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(foreground, None)
        };
        let attached =
            owner != 0 && owner != current && AttachThreadInput(current, owner, true).as_bool();
        let focused = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(current, owner, false);
        }
        focused || GetForegroundWindow() == hwnd
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn lookup_candidates(pid: u32) -> Vec<WindowCandidate> {
    let pid = pid.to_string();
    let Some(all) = xdotool(&["search", "--pid", &pid]) else {
        tracing::debug!("xdotool could not list the game windows");
        return Vec::new();
    };
    let mapped: Vec<String> = xdotool(&["search", "--onlyvisible", "--pid", &pid])
        .map(|output| output.lines().map(str::trim).map(str::to_string).collect())
        .unwrap_or_default();
    all.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|id| WindowCandidate {
            id: id.to_string(),
            visible: mapped.iter().any(|mapped_id| mapped_id.as_str() == id),
            // An unmapped X11 window is either iconified or hidden; `windowmap`
            // restores both, so it is treated as minimised.
            minimized: !mapped.iter().any(|mapped_id| mapped_id.as_str() == id),
        })
        .collect()
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

/// macOS: no window-by-PID lookup exists here (see the module docs).
#[cfg(target_os = "macos")]
fn lookup_candidates(_pid: u32) -> Vec<WindowCandidate> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn focus_window(_id: &str) -> bool {
    false
}

/// Look up the PID's window and focus it, in one step.
pub fn lookup_and_focus(pid: u32) -> FocusOutcome {
    focus_with(|| lookup_candidates(pid), focus_window)
}

/* ---------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str, visible: bool, minimized: bool) -> WindowCandidate {
        WindowCandidate {
            id: id.to_string(),
            visible,
            minimized,
        }
    }

    fn focused(id: &str) -> FocusOutcome {
        FocusOutcome::Focused {
            window: id.to_string(),
        }
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
        let focused_ids = std::cell::RefCell::new(Vec::new());
        let outcome = focus_with(
            || vec![window("42", true, false)],
            |id| {
                focused_ids.borrow_mut().push(id.to_string());
                true
            },
        );
        assert_eq!(outcome, focused("42"));
        assert_eq!(focused_ids.into_inner(), vec!["42".to_string()]);
    }

    #[test]
    fn no_window_falls_back_to_alt_tab() {
        let outcome = focus_with(Vec::new, |_| panic!("nothing to focus"));
        assert_eq!(outcome, FocusOutcome::NoWindow);
    }

    #[test]
    fn a_refused_focus_is_reported_for_the_alt_tab_fallback() {
        let outcome = focus_with(|| vec![window("7", true, false)], |_| false);
        assert_eq!(
            outcome,
            FocusOutcome::Refused {
                window: "7".to_string()
            }
        );
    }

    /* ------------------------------ the ladder ----------------------------- */

    #[test]
    fn a_plain_request_that_works_stops_the_ladder() {
        let tried = std::cell::RefCell::new(Vec::new());
        let winner = focus_ladder(|step| {
            tried.borrow_mut().push(step);
            true
        });
        assert_eq!(winner, Some(FocusStep::Request));
        assert_eq!(tried.into_inner(), vec![FocusStep::Request]);
    }

    #[test]
    fn a_refused_request_falls_back_to_the_foreground_lock() {
        let tried = std::cell::RefCell::new(Vec::new());
        let winner = focus_ladder(|step| {
            tried.borrow_mut().push(step);
            step == FocusStep::ForegroundLock
        });
        assert_eq!(winner, Some(FocusStep::ForegroundLock));
        assert_eq!(
            tried.into_inner(),
            vec![FocusStep::Request, FocusStep::ForegroundLock]
        );
    }

    #[test]
    fn the_last_rung_is_bring_to_top_and_every_refusal_is_reported() {
        let tried = std::cell::RefCell::new(Vec::new());
        let winner = focus_ladder(|step| {
            tried.borrow_mut().push(step);
            step == FocusStep::BringToTop
        });
        assert_eq!(winner, Some(FocusStep::BringToTop));
        assert_eq!(
            tried.into_inner(),
            vec![
                FocusStep::Request,
                FocusStep::ForegroundLock,
                FocusStep::BringToTop
            ]
        );

        let refused = focus_ladder(|_| false);
        assert_eq!(refused, None);
    }

    /* --------------------------- waiting for a window ----------------------- */

    #[test]
    fn the_budget_covers_the_documented_window() {
        assert_eq!(
            focus_attempts(LAUNCH_FOCUS_BUDGET, LAUNCH_FOCUS_INTERVAL),
            120
        );
        // Degenerate inputs never spin forever or divide by zero.
        assert_eq!(focus_attempts(Duration::ZERO, LAUNCH_FOCUS_INTERVAL), 1);
        assert_eq!(focus_attempts(LAUNCH_FOCUS_BUDGET, Duration::ZERO), 1);
        assert_eq!(
            focus_attempts(Duration::from_millis(120), LAUNCH_FOCUS_INTERVAL),
            1
        );
    }

    #[test]
    fn a_window_found_on_the_first_attempt_is_focused_without_waiting() {
        let waits = std::cell::Cell::new(0);
        let outcome = wait_for_window(120, || focused("11"), || waits.set(waits.get() + 1));
        assert_eq!(outcome, focused("11"));
        assert_eq!(waits.get(), 0);
    }

    #[test]
    fn a_window_found_after_a_delay_is_focused_within_the_budget() {
        let attempts = std::cell::Cell::new(0);
        let waits = std::cell::Cell::new(0);
        let outcome = wait_for_window(
            120,
            || {
                attempts.set(attempts.get() + 1);
                // A splash screen first, then the real window on the third poll.
                if attempts.get() < 3 {
                    FocusOutcome::NoWindow
                } else {
                    focused("12")
                }
            },
            || waits.set(waits.get() + 1),
        );
        assert_eq!(outcome, focused("12"));
        assert_eq!(attempts.get(), 3);
        assert_eq!(waits.get(), 2);
    }

    #[test]
    fn a_game_without_a_window_never_focuses_and_stops_at_the_budget() {
        let attempts = std::cell::Cell::new(0);
        let waits = std::cell::Cell::new(0);
        let outcome = wait_for_window(
            4,
            || {
                attempts.set(attempts.get() + 1);
                FocusOutcome::NoWindow
            },
            || waits.set(waits.get() + 1),
        );
        // No window means no focus call at all: another app keeps the focus.
        assert_eq!(outcome, FocusOutcome::NoWindow);
        assert_eq!(attempts.get(), 4);
        assert_eq!(waits.get(), 3);
    }

    #[test]
    fn a_refused_window_ends_the_wait_instead_of_focusing_again() {
        let attempts = std::cell::Cell::new(0);
        let outcome = wait_for_window(
            120,
            || {
                attempts.set(attempts.get() + 1);
                FocusOutcome::Refused {
                    window: "13".to_string(),
                }
            },
            || panic!("a refused focus is final"),
        );
        assert_eq!(
            outcome,
            FocusOutcome::Refused {
                window: "13".to_string()
            }
        );
        assert_eq!(attempts.get(), 1);
    }

    /* ---------------------------- platform limits --------------------------- */

    #[test]
    fn the_platform_capability_matches_the_documented_limit() {
        // macOS has no window-by-PID lookup, so focus-on-launch is a no-op there
        // (see the module docs and `docs/CONTRACT.md`).
        #[cfg(target_os = "macos")]
        assert!(!focus_supported());
        // Windows focuses through the ladder, X11 through xdotool. xdotool may
        // be missing at runtime: that is reported, never fatal.
        #[cfg(not(target_os = "macos"))]
        assert!(focus_supported());
    }

    #[test]
    fn the_lookup_on_this_platform_never_panics() {
        // A PID that cannot own a window: the lookup returns no candidate
        // instead of failing, which is the Alt+Tab path.
        let outcome = lookup_and_focus(u32::MAX);
        assert_eq!(outcome, FocusOutcome::NoWindow);
    }
}
