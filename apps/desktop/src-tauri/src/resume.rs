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
use crate::window_focus::{lookup_and_focus, FocusOutcome};

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

// The window lookup, the window preference and the focus ladder live in
// `window_focus.rs` (#219): the launch path uses the same primitives, so there
// is exactly one `EnumWindows`/`xdotool` lookup and one focus ladder per
// platform. Resume only decides what a failed focus means for the shell.

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
        FocusOutcome::Focused { window } => {
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
        FocusOutcome::NoWindow => {
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
        FocusOutcome::Refused { window } => {
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
}
