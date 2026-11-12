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

pub fn start(app: AppHandle) {
    let thread_app = app.clone();
    let result = std::thread::Builder::new()
        .name("ludex-overlay-gamepad".to_string())
        .spawn(move || run(thread_app));
    if let Err(error) = result {
        tracing::warn!(error = %error, "could not start the global gamepad listener");
        app.state::<AppState>()
            .overlay
            .set_listener_status(OverlayListenerStatus {
                backend: "failed".to_string(),
                available: false,
                platform: std::env::consts::OS.to_string(),
                reason: Some(format!(
                    "The controller listener thread could not start: {error}"
                )),
                controllers: Vec::new(),
            });
    }
}

fn run(app: AppHandle) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(error) => {
            tracing::warn!(error = %error, "the global gamepad listener is unavailable");
            app.state::<AppState>()
                .overlay
                .set_listener_status(OverlayListenerStatus {
                    backend: "failed".to_string(),
                    available: false,
                    platform: std::env::consts::OS.to_string(),
                    reason: Some(format!(
                        "Controllers cannot be read on this system: {error}"
                    )),
                    controllers: Vec::new(),
                });
            return;
        }
    };

    let trigger = current_trigger(&app);
    let mut detector = GestureDetector::new(DetectorConfig::for_trigger(overlay::trigger_choice(
        trigger,
    )));
    let started = Instant::now();
    let mut last_config_check = Instant::now();
    let mut last_status_publish = Instant::now() - STATUS_REFRESH;
    let mut active_pad: Option<usize> = None;
    let mut active_at: Option<Instant> = None;

    publish_controllers(&app, &gilrs, active_pad, active_at);

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
                    if let Some(button) = pad_button(button) {
                        if detector.handle(PadEvent::Pressed(button), now_ms)
                            == Some(Gesture::Toggle)
                        {
                            overlay::toggle(&app);
                        }
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(button) = pad_button(button) {
                        detector.handle(PadEvent::Released(button), now_ms);
                    }
                }
                EventType::AxisChanged(_, value, _) => {
                    if axis_active(value, DEFAULT_DEADZONE) {
                        active_pad = Some(pad);
                        active_at = Some(Instant::now());
                    }
                }
                EventType::Connected => {
                    // A new pad may arrive with buttons already held; never
                    // inherit a half-pressed chord from the previous device.
                    detector.reset();
                    last_status_publish = Instant::now() - STATUS_REFRESH;
                    tracing::info!(pad, "controller connected to the overlay listener");
                }
                EventType::Disconnected => {
                    detector.reset();
                    if active_pad == Some(pad) {
                        active_pad = None;
                        active_at = None;
                    }
                    last_status_publish = Instant::now() - STATUS_REFRESH;
                    tracing::info!(pad, "controller disconnected from the overlay listener");
                }
                _ => {}
            }
        }

        if last_config_check.elapsed() >= CONFIG_REFRESH {
            last_config_check = Instant::now();
            let trigger = current_trigger(&app);
            let config = DetectorConfig::for_trigger(overlay::trigger_choice(trigger));
            if config != detector.config() {
                detector.set_config(config);
            }
        }

        if last_status_publish.elapsed() >= STATUS_REFRESH {
            last_status_publish = Instant::now();
            publish_controllers(&app, &gilrs, active_pad, active_at);
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
}
