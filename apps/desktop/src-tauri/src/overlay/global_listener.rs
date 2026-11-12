//! Native global gamepad listener (#144 phase 2).
//!
//! Runs on its own thread and reads controller buttons through `gilrs`, which
//! stays connected to the device even while another window (a game) owns the
//! keyboard/mouse focus. The thread never injects anything into a game: it only
//! opens/closes Ludex's own overlay window.
//!
//! Supported on Windows (`xinput`/`wgi` backends) and Linux (`evdev`/udev, which
//! needs read access to `/dev/input/event*` — usually the `input` group, or a
//! udev rule). macOS uses the in-app overlay only.
//!
//! The thread is a supervisor: `Gilrs::new()` failures (no udev permission, a
//! driver that is not ready yet) are retried with a capped backoff instead of
//! killing the listener until the next app restart. Hot-plug edges reset the
//! gesture detector, so a new pad can never inherit a half-pressed chord.

use std::time::{Duration, Instant};

use gilrs::{Button, EventType, Gilrs, PowerInfo};
use tauri::{AppHandle, Manager};

use crate::overlay::detector::{
    axis_active, DetectorConfig, Gesture, GestureDetector, PadButton, PadEvent, DEFAULT_DEADZONE,
};
use crate::overlay::{self, OverlayController, OverlayListenerStatus};
use crate::state::AppState;

/// How often the settings trigger is re-read while the listener idles.
const CONFIG_REFRESH: Duration = Duration::from_secs(2);
/// How often the controller list (name, battery, active pad) is republished.
const STATUS_REFRESH: Duration = Duration::from_secs(2);
/// A pad counts as "active" for this long after its last input.
const ACTIVITY_WINDOW: Duration = Duration::from_secs(5);
/// Blocking wait so the loop can still refresh status while no pad moves.
const POLL_TIMEOUT: Duration = Duration::from_millis(750);
/// Never retry an unavailable controller backend more slowly than this.
const MAX_RETRY_SECS: u64 = 30;

pub fn start(app: AppHandle) {
    let state = app.state::<AppState>();
    if state.overlay.listener_running() {
        return;
    }
    state.overlay.set_listener_running(true);

    let thread_app = app.clone();
    let result = std::thread::Builder::new()
        .name("ludex-overlay-gamepad".to_string())
        .spawn(move || run(thread_app));
    if let Err(error) = result {
        app.state::<AppState>().overlay.set_listener_running(false);
        publish_failure(
            &app,
            format!("The controller listener thread could not start: {error}"),
        );
        tracing::warn!(error = %error, "could not start the global gamepad listener");
    }
}

/// Supervisor: retry `Gilrs::new()` instead of giving up for the whole session.
fn run(app: AppHandle) {
    let mut attempt = 0u32;
    loop {
        match Gilrs::new() {
            Ok(gilrs) => {
                if attempt > 0 {
                    tracing::info!(attempts = attempt, "the global gamepad listener recovered");
                }
                publish_controllers(&app, &gilrs, None, None);
                run_events(&app, gilrs);
                // `run_events` only returns when the process is shutting down.
                app.state::<AppState>().overlay.set_listener_running(false);
                return;
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                let delay = retry_delay(attempt);
                publish_failure(
                    &app,
                    format!(
                        "Controllers cannot be read on this system: {error}. Retrying in {}s.",
                        delay.as_secs()
                    ),
                );
                tracing::warn!(
                    attempt,
                    error = %error,
                    retry_in_secs = delay.as_secs(),
                    "the global gamepad listener is unavailable; retrying"
                );
                std::thread::sleep(delay);
            }
        }
    }
}

/// Capped exponential backoff: 1, 2, 4, 8, 16, then 30 seconds.
pub fn retry_delay(attempt: u32) -> Duration {
    const BACKOFF_SECS: [u64; 5] = [1, 2, 4, 8, 16];
    let index = usize::try_from(attempt.saturating_sub(1)).unwrap_or(usize::MAX);
    let seconds = BACKOFF_SECS.get(index).copied().unwrap_or(MAX_RETRY_SECS);
    Duration::from_secs(seconds)
}

/// A pad arrives or leaves while the listener runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotplugEdge {
    Connected,
    Disconnected,
}

/// What a hot-plug edge must do. Pure so the re-arm rule is unit-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotplugPlan {
    /// Drop held buttons: a new pad may arrive with the chord held.
    pub reset_detector: bool,
    /// Forget the active-pad indicator when that pad leaves.
    pub clear_active_pad: bool,
    /// Republish the controller list immediately.
    pub republish: bool,
}

pub fn hotplug_plan(edge: HotplugEdge) -> HotplugPlan {
    HotplugPlan {
        reset_detector: true,
        clear_active_pad: edge == HotplugEdge::Disconnected,
        republish: true,
    }
}

fn publish_failure(app: &AppHandle, reason: String) {
    app.state::<AppState>()
        .overlay
        .set_listener_status(OverlayListenerStatus {
            backend: "failed".to_string(),
            available: false,
            running: true,
            platform: std::env::consts::OS.to_string(),
            reason: Some(reason),
            controllers: Vec::new(),
        });
}

fn run_events(app: &AppHandle, mut gilrs: Gilrs) {
    let trigger = current_trigger(app);
    let mut detector = GestureDetector::new(DetectorConfig::for_trigger(overlay::trigger_choice(
        trigger,
    )));
    let started = Instant::now();
    let mut last_config_check = Instant::now();
    let mut last_status_publish = Instant::now() - STATUS_REFRESH;
    let mut active_pad: Option<usize> = None;
    let mut active_at: Option<Instant> = None;

    loop {
        let event = gilrs.next_event_blocking(Some(POLL_TIMEOUT));
        if let Some(event) = event {
            let now_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let pad = usize::from(event.id);
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if is_overlay_button(button) {
                        active_pad = Some(pad);
                        active_at = Some(Instant::now());
                    }
                    if let Some(mapped) = pad_button(button) {
                        tracing::info!(pad, button = ?mapped, "overlay button pressed");
                        if detector.handle(PadEvent::Pressed(mapped), now_ms)
                            == Some(Gesture::Toggle)
                        {
                            tracing::info!(pad, "overlay gesture recognised");
                            overlay::toggle(app, "controller");
                        }
                    } else {
                        tracing::trace!(pad, button = ?button, "controller button pressed");
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(mapped) = pad_button(button) {
                        tracing::info!(pad, button = ?mapped, "overlay button released");
                        detector.handle(PadEvent::Released(mapped), now_ms);
                    }
                }
                EventType::AxisChanged(_, value, _) => {
                    if axis_active(value, DEFAULT_DEADZONE) {
                        active_pad = Some(pad);
                        active_at = Some(Instant::now());
                    }
                }
                EventType::Connected => {
                    let plan = hotplug_plan(HotplugEdge::Connected);
                    if plan.reset_detector {
                        detector.reset();
                    }
                    if plan.republish {
                        last_status_publish = Instant::now() - STATUS_REFRESH;
                    }
                    tracing::info!(pad, "controller connected; overlay trigger re-armed");
                }
                EventType::Disconnected => {
                    let plan = hotplug_plan(HotplugEdge::Disconnected);
                    if plan.reset_detector {
                        detector.reset();
                    }
                    if plan.clear_active_pad && active_pad == Some(pad) {
                        active_pad = None;
                        active_at = None;
                    }
                    if plan.republish {
                        last_status_publish = Instant::now() - STATUS_REFRESH;
                    }
                    tracing::info!(pad, "controller disconnected; overlay trigger re-armed");
                }
                _ => {}
            }
        }

        if last_config_check.elapsed() >= CONFIG_REFRESH {
            last_config_check = Instant::now();
            let trigger = current_trigger(app);
            let config = DetectorConfig::for_trigger(overlay::trigger_choice(trigger));
            if config != detector.config() {
                tracing::info!(?trigger, "overlay trigger setting changed");
                detector.set_config(config);
            }
        }

        if last_status_publish.elapsed() >= STATUS_REFRESH {
            last_status_publish = Instant::now();
            publish_controllers(app, &gilrs, active_pad, active_at);
        }
    }
}

fn current_trigger(app: &AppHandle) -> crate::settings::OverlayTrigger {
    app.state::<AppState>().settings_snapshot().overlay_trigger
}

fn is_overlay_button(button: Button) -> bool {
    matches!(
        button,
        Button::Mode | Button::LeftTrigger | Button::RightTrigger | Button::Start
    )
}

/// Buttons the overlay understands. Bumpers are `BTN_TL`/`BTN_TR`, which gilrs
/// calls `LeftTrigger`/`RightTrigger`; the analog triggers are `*Trigger2` and
/// deliberately stay out of the chord so a trigger pull never opens the overlay.
fn pad_button(button: Button) -> Option<PadButton> {
    match button {
        Button::Mode => Some(PadButton::Guide),
        Button::LeftTrigger => Some(PadButton::LeftBumper),
        Button::RightTrigger => Some(PadButton::RightBumper),
        Button::Start => Some(PadButton::Start),
        _ => None,
    }
}

fn power_info(info: PowerInfo) -> (Option<u8>, bool) {
    match info {
        PowerInfo::Charging(percent) => (Some(percent), true),
        PowerInfo::Discharging(percent) => (Some(percent), false),
        PowerInfo::Charged => (Some(100), true),
        // Wired, Unknown, and any future variant the crate adds.
        _ => (None, false),
    }
}

fn publish_controllers(
    app: &AppHandle,
    gilrs: &Gilrs,
    active_pad: Option<usize>,
    active_at: Option<Instant>,
) {
    let now = Instant::now();
    let active_pad = active_pad.filter(|_| {
        active_at
            .map(|at| now.duration_since(at) < ACTIVITY_WINDOW)
            .unwrap_or(false)
    });

    let mut controllers: Vec<OverlayController> = gilrs
        .gamepads()
        .filter(|(_, gamepad)| gamepad.is_connected())
        .map(|(id, gamepad)| {
            let (battery, charging) = power_info(gamepad.power_info());
            let pad = usize::from(id);
            OverlayController {
                id: pad.to_string(),
                name: gamepad.name().to_string(),
                active: active_pad == Some(pad),
                battery,
                charging,
            }
        })
        .collect();
    controllers.sort_by(|left, right| left.name.cmp(&right.name));

    app.state::<AppState>()
        .overlay
        .set_listener_status(OverlayListenerStatus {
            backend: "gilrs".to_string(),
            available: true,
            running: true,
            platform: std::env::consts::OS.to_string(),
            reason: None,
            controllers,
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_overlay_buttons_are_mapped() {
        assert_eq!(pad_button(Button::Mode), Some(PadButton::Guide));
        assert_eq!(pad_button(Button::LeftTrigger), Some(PadButton::LeftBumper));
        assert_eq!(
            pad_button(Button::RightTrigger),
            Some(PadButton::RightBumper)
        );
        assert_eq!(pad_button(Button::Start), Some(PadButton::Start));
        assert_eq!(pad_button(Button::South), None);
        // Analog triggers must not trigger the chord.
        assert_eq!(pad_button(Button::LeftTrigger2), None);
        assert_eq!(pad_button(Button::RightTrigger2), None);
    }

    #[test]
    fn battery_information_is_reported_without_panic() {
        assert_eq!(power_info(PowerInfo::Charging(80)), (Some(80), true));
        assert_eq!(power_info(PowerInfo::Discharging(55)), (Some(55), false));
        assert_eq!(power_info(PowerInfo::Charged), (Some(100), true));
        assert_eq!(power_info(PowerInfo::Wired), (None, false));
        assert_eq!(power_info(PowerInfo::Unknown), (None, false));
    }

    #[test]
    fn listener_restarts_with_a_capped_backoff() {
        // A transient driver/permission error must retry quickly, then settle
        // on a slow poll rather than spinning or giving up for the session.
        let expected = [1, 2, 4, 8, 16, 30, 30];
        for (attempt, seconds) in expected.into_iter().enumerate() {
            assert_eq!(
                retry_delay(attempt as u32 + 1),
                Duration::from_secs(seconds),
                "attempt {}",
                attempt + 1
            );
        }
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(u32::MAX), Duration::from_secs(MAX_RETRY_SECS));
    }

    #[test]
    fn hot_plug_edges_re_arm_the_trigger() {
        // A pad that arrives with the chord held must start from a clean
        // detector, and the panel must see the new controller immediately.
        let connected = hotplug_plan(HotplugEdge::Connected);
        assert!(connected.reset_detector);
        assert!(connected.republish);
        assert!(!connected.clear_active_pad);

        let disconnected = hotplug_plan(HotplugEdge::Disconnected);
        assert!(disconnected.reset_detector);
        assert!(disconnected.republish);
        assert!(disconnected.clear_active_pad);
    }
}
