//! Native power commands (#196/#269): quit the app and sleep the PC.
//!
//! Both were invoked by the web but missing from the Tauri registry, so the
//! Console Power rows could only toast "Not available". This module adds the
//! commands plus the two things that make them honest:
//!
//! - a capability probe (`sleep_capability`) so a row is disabled with a reason
//!   instead of failing when pressed;
//! - typed refusals ([`PowerError`]) while a game runs (the stream would drop
//!   mid-session) or while installs are active (sleep stops the transfers until
//!   the machine wakes). Active installs need an explicit confirmation; the
//!   Console confirmation modal already defaults to Cancel.
//!
//! Sleep is fire-and-wait by nature: `SetSuspendState`, `systemctl suspend` and
//! `pmset sleepnow` return when the machine resumes, so the blocking call runs
//! on a blocking thread and the UI never waits on it.

use std::process::Command;

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::install::InstallPhase;
use crate::state::AppState;

/// Error codes the web can branch on. Kept as strings so a newer desktop can
/// add one without a web release.
pub const CODE_UNSUPPORTED: &str = "unsupported";
pub const CODE_GAME_RUNNING: &str = "gameRunning";
pub const CODE_INSTALLS_ACTIVE: &str = "installsActive";
pub const CODE_FAILED: &str = "failed";

/// A refused power action. Serialised as `{code, message}`; the message is the
/// sentence the row or the toast shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerError {
    pub code: String,
    pub message: String,
}

impl PowerError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PowerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Whether this machine can suspend, and through what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepCapability {
    pub available: bool,
    /// The OS call or command the row promises, for the diagnostics bundle.
    pub backend: Option<String>,
    /// Why it is unavailable; the Power row shows it instead of a toast.
    pub reason: Option<String>,
}

impl SleepCapability {
    fn available(backend: &str) -> Self {
        Self {
            available: true,
            backend: Some(backend.to_string()),
            reason: None,
        }
    }

    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            backend: None,
            reason: Some(reason.into()),
        }
    }
}

/// What the OS probe found. Pure input for [`capability_for`] so the mapping is
/// tested on every host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SleepProbe {
    /// The platform's sleep tool exists (Linux `systemctl`, macOS `pmset`).
    pub command_present: bool,
    /// Linux: `systemctl can-suspend`; `None` when the probe could not run.
    pub can_suspend: Option<bool>,
}

/// Map a probe result to a capability. The Windows row needs no probe: every
/// supported Windows ships `SetSuspendState`.
pub fn capability_for(
    platform: crate::window_policy::Platform,
    probe: SleepProbe,
) -> SleepCapability {
    use crate::window_policy::Platform;
    match platform {
        Platform::Windows => SleepCapability::available("SetSuspendState"),
        Platform::MacOs => {
            if probe.command_present {
                SleepCapability::available("pmset sleepnow")
            } else {
                SleepCapability::unavailable(
                    "`pmset` was not found; Ludex cannot put this Mac to sleep.",
                )
            }
        }
        Platform::Linux => {
            if !probe.command_present {
                SleepCapability::unavailable(
                    "Sleep needs systemd-logind, and `systemctl` was not found on this machine.",
                )
            } else if probe.can_suspend == Some(false) {
                SleepCapability::unavailable(
                    "systemd-logind reports that this machine cannot suspend.",
                )
            } else {
                SleepCapability::available("systemctl suspend")
            }
        }
        Platform::Other => {
            SleepCapability::unavailable("Sleep is not implemented on this platform.")
        }
    }
}

/// True when `name` is an executable on `PATH`. Avoids a `which` dependency.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|directory| directory.join(name).is_file()))
        .unwrap_or(false)
}

/// Linux: `systemctl can-suspend` prints `yes`/`no` (systemd 203+).
fn systemctl_can_suspend() -> Option<bool> {
    let output = Command::new("systemctl").arg("can-suspend").output().ok()?;
    let answer = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_ascii_lowercase();
    if answer == "yes" {
        Some(true)
    } else if answer == "no" {
        Some(false)
    } else if output.status.success() {
        // Older builds print nothing but exit 0 when the machine can suspend.
        Some(true)
    } else {
        Some(false)
    }
}

/// Probe the running machine.
pub fn probe() -> SleepProbe {
    use crate::window_policy::Platform;
    match crate::window_policy::current_platform() {
        Platform::Linux => SleepProbe {
            command_present: on_path("systemctl"),
            can_suspend: if on_path("systemctl") {
                systemctl_can_suspend()
            } else {
                None
            },
        },
        Platform::MacOs => SleepProbe {
            // pmset lives in /usr/bin on every macOS; PATH is a fallback for
            // unusual shells.
            command_present: std::path::Path::new("/usr/bin/pmset").is_file() || on_path("pmset"),
            can_suspend: None,
        },
        _ => SleepProbe::default(),
    }
}

/// The capability of the running machine.
pub fn capability() -> SleepCapability {
    capability_for(crate::window_policy::current_platform(), probe())
}

/// What blocks a sleep request right now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SleepBlockers {
    /// Title of the game still running, when one is.
    pub running_game: Option<String>,
    /// Installs queued or in flight (paused jobs do not count).
    pub active_installs: u32,
}

/// Decide whether `system_sleep` may suspend. Pure so the matrix is tested
/// without suspending the test machine.
pub fn sleep_plan(
    capability: &SleepCapability,
    blockers: &SleepBlockers,
    confirm_active_installs: bool,
) -> Result<(), PowerError> {
    if !capability.available {
        return Err(PowerError::new(
            CODE_UNSUPPORTED,
            capability
                .reason
                .clone()
                .unwrap_or_else(|| "This machine cannot suspend.".to_string()),
        ));
    }
    if let Some(title) = blockers.running_game.as_deref() {
        return Err(PowerError::new(
            CODE_GAME_RUNNING,
            format!("Quit {title} before sleeping this PC."),
        ));
    }
    if blockers.active_installs > 0 && !confirm_active_installs {
        let installs = blockers.active_installs;
        return Err(PowerError::new(
            CODE_INSTALLS_ACTIVE,
            format!(
                "{installs} install(s) are still active. Sleeping pauses them until you wake the PC."
            ),
        ));
    }
    Ok(())
}

/// Titles of running launches are indexed by game id; the newest one is the
/// game the player is in.
fn running_game_title(app: &AppHandle) -> Option<String> {
    let state = app.try_state::<AppState>()?;
    let game_id = {
        let processes = state.processes.lock().ok()?;
        processes
            .iter()
            .max_by_key(|(_, entry)| entry.started_at)
            .map(|(_, entry)| entry.game_id)?
    };
    // Release the process lock before touching SQLite (same lock order as the
    // overlay and the launcher).
    let connection = state.db.lock().ok()?;
    crate::db::get_installed(&connection, game_id)
        .ok()
        .flatten()
        .map(|installed| installed.title)
}

/// Count the jobs a sleep would interrupt.
pub fn active_installs(app: &AppHandle) -> u32 {
    crate::install::list(app)
        .map(|jobs| {
            jobs.iter()
                .filter(|job| !job.phase.is_terminal() && job.phase != InstallPhase::Paused)
                .count() as u32
        })
        .unwrap_or(0)
}

fn blockers(app: &AppHandle) -> SleepBlockers {
    SleepBlockers {
        running_game: running_game_title(app),
        active_installs: active_installs(app),
    }
}

/* ------------------------------ platform sleep --------------------------- */

// `SetSuspendState(FALSE, FALSE, FALSE)` is the documented suspend request.
// Note that Windows hibernates instead when hibernation is enabled on the
// machine; there is no flag to force a plain sleep.
#[cfg(target_os = "windows")]
#[link(name = "powrprof")]
unsafe extern "system" {
    fn SetSuspendState(hibernate: i32, force_critical: i32, disable_wake_event: i32) -> i32;
}

#[cfg(target_os = "windows")]
fn suspend() -> Result<(), String> {
    // SAFETY: powrprof is an OS library, the three arguments are plain BOOLEAN
    // values (0/1) and no pointer crosses the call.
    let accepted = unsafe { SetSuspendState(0, 0, 0) };
    if accepted == 0 {
        Err("Windows refused the suspend request.".to_string())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn suspend() -> Result<(), String> {
    let status = Command::new("systemctl")
        .arg("suspend")
        .status()
        .map_err(|error| format!("Could not start `systemctl suspend`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`systemctl suspend` exited with {status}."))
    }
}

#[cfg(target_os = "macos")]
fn suspend() -> Result<(), String> {
    let status = Command::new("/usr/bin/pmset")
        .arg("sleepnow")
        .status()
        .map_err(|error| format!("Could not start `pmset sleepnow`: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`pmset sleepnow` exited with {status}."))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn suspend() -> Result<(), String> {
    Err("Sleep is not implemented on this platform.".to_string())
}

/* -------------------------------- commands ------------------------------- */

/// Quit the whole app (#269). Installs are resumable, so nothing is blocked on
/// them; the exit handler still clears the resume markers and stops the
/// embedded server (`lib.rs`, `RunEvent::Exit`).
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    tracing::info!("quit requested from the console shell");
    app.exit(0);
}

/// The sleep capability of this machine, for the Power row. The probe spawns
/// `systemctl can-suspend` on Linux, so it never blocks the main thread.
#[tauri::command]
pub async fn sleep_capability() -> SleepCapability {
    tokio::task::spawn_blocking(capability)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(error = %error, "the sleep capability probe panicked");
            SleepCapability::unavailable("The sleep probe failed. Try again.")
        })
}

/// Suspend the PC. Refused with a typed error while a game runs or while active
/// installs were not confirmed; the OS call blocks until the machine resumes,
/// so it never runs on the async runtime.
#[tauri::command]
pub async fn system_sleep(
    app: AppHandle,
    confirm_active_installs: Option<bool>,
) -> Result<(), PowerError> {
    let capability = tokio::task::spawn_blocking(capability)
        .await
        .map_err(|error| {
            PowerError::new(CODE_FAILED, format!("The sleep probe failed: {error}"))
        })?;

    let blocker_handle = app.clone();
    let blockers = tokio::task::spawn_blocking(move || blockers(&blocker_handle))
        .await
        .map_err(|error| {
            PowerError::new(CODE_FAILED, format!("Could not read running work: {error}"))
        })?;

    sleep_plan(
        &capability,
        &blockers,
        confirm_active_installs.unwrap_or(false),
    )?;

    tracing::info!(
        backend = capability.backend.as_deref().unwrap_or("unknown"),
        installs = blockers.active_installs,
        "sleeping the PC"
    );
    tokio::task::spawn_blocking(suspend)
        .await
        .map_err(|error| PowerError::new(CODE_FAILED, format!("The sleep call failed: {error}")))?
        .map_err(|message| PowerError::new(CODE_FAILED, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_policy::Platform;

    fn probe(present: bool, can_suspend: Option<bool>) -> SleepProbe {
        SleepProbe {
            command_present: present,
            can_suspend,
        }
    }

    #[test]
    fn the_capability_matrix_is_honest_per_platform() {
        // Windows has no probe: SetSuspendState ships with the OS.
        let windows = capability_for(Platform::Windows, SleepProbe::default());
        assert!(windows.available);
        assert_eq!(windows.backend.as_deref(), Some("SetSuspendState"));
        assert!(windows.reason.is_none());

        // macOS needs pmset.
        assert!(capability_for(Platform::MacOs, probe(true, None)).available);
        let mac_without_pmset = capability_for(Platform::MacOs, probe(false, None));
        assert!(!mac_without_pmset.available);
        assert!(mac_without_pmset.reason.unwrap().contains("pmset"));

        // Linux needs logind, and an explicit "no" disables the row.
        let linux = capability_for(Platform::Linux, probe(true, Some(true)));
        assert!(linux.available);
        assert_eq!(linux.backend.as_deref(), Some("systemctl suspend"));
        let no_logind = capability_for(Platform::Linux, probe(false, None));
        assert!(!no_logind.available);
        assert!(no_logind.reason.unwrap().contains("systemd-logind"));
        let cannot = capability_for(Platform::Linux, probe(true, Some(false)));
        assert!(!cannot.available);
        assert!(cannot.reason.unwrap().contains("cannot suspend"));

        // A failed probe (None) stays optimistic: pressing the row surfaces the
        // real error instead of hiding a working machine.
        assert!(capability_for(Platform::Linux, probe(true, None)).available);

        assert!(!capability_for(Platform::Other, SleepProbe::default()).available);
    }

    #[test]
    fn sleep_is_refused_per_blocker() {
        let capable = SleepCapability::available("systemctl suspend");
        let unavailable = SleepCapability::unavailable("systemd-logind says no.");

        let error = sleep_plan(&unavailable, &SleepBlockers::default(), true).unwrap_err();
        assert_eq!(error.code, CODE_UNSUPPORTED);
        assert_eq!(error.message, "systemd-logind says no.");

        let running = SleepBlockers {
            running_game: Some("Hades II".to_string()),
            active_installs: 0,
        };
        let error = sleep_plan(&capable, &running, true).unwrap_err();
        assert_eq!(error.code, CODE_GAME_RUNNING);
        assert_eq!(error.message, "Quit Hades II before sleeping this PC.");

        // Active installs need the explicit confirmation that the Console
        // modal (default Cancel) provides.
        let installing = SleepBlockers {
            running_game: None,
            active_installs: 2,
        };
        let error = sleep_plan(&capable, &installing, false).unwrap_err();
        assert_eq!(error.code, CODE_INSTALLS_ACTIVE);
        assert!(error.message.contains("2 install(s)"));
        assert!(sleep_plan(&capable, &installing, true).is_ok());

        assert!(sleep_plan(&capable, &SleepBlockers::default(), false).is_ok());
    }

    #[test]
    fn the_command_shape_is_camel_case_for_the_web() {
        let capability = SleepCapability::available("pmset sleepnow");
        let value = serde_json::to_value(capability).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "available": true,
                "backend": "pmset sleepnow",
                "reason": null,
            })
        );
        let error = PowerError::new(CODE_INSTALLS_ACTIVE, "2 install(s) are still active.");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "code": "installsActive",
                "message": "2 install(s) are still active.",
            })
        );
        // The web's errorMessage() reads `message`; Display keeps the log line
        // readable too.
        assert_eq!(error.to_string(), "2 install(s) are still active.");
    }
}
