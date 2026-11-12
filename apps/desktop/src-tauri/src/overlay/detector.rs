//! Pure gesture detection for the in-game overlay (#144).
//!
//! The native listener (`overlay::global_listener`) turns `gilrs` button events
//! into [`PadEvent`]s and feeds this detector. Keeping the detection free of
//! platform types is what makes the debounce testable with a fake pad: no
//! controller, no udev, no thread.
//!
//! Two gestures are recognised:
//!
//! - **double Guide/Home/PS press** inside [`DOUBLE_PRESS_MS`]. A single press
//!   never produces anything, so the OS/Steam menu keeps working;
//! - **LB+RB+Start chord**, for pads whose Guide button is not exposed to the
//!   host (XInput/WGI on Windows do not report it).
//!
//! A short cooldown between gestures keeps one physical gesture from toggling
//! the overlay twice when a pad reports duplicate edges.

/// Buttons the overlay recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadButton {
    /// Guide on Xbox, Home on DualSense, PS on DualShock.
    Guide,
    /// Left bumper (`BTN_TL`), not the analog trigger.
    LeftBumper,
    /// Right bumper (`BTN_TR`).
    RightBumper,
    Start,
}

impl PadButton {
    fn index(self) -> usize {
        match self {
            PadButton::Guide => 0,
            PadButton::LeftBumper => 1,
            PadButton::RightBumper => 2,
            PadButton::Start => 3,
        }
    }
}

/// One button edge from the native listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadEvent {
    Pressed(PadButton),
    Released(PadButton),
}

/// What the caller should do with the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Open the overlay when it is closed, close it when it is open.
    Toggle,
}

/// Default window for the double press, in milliseconds.
pub const DOUBLE_PRESS_MS: u64 = 400;
/// Minimum delay between two recognised gestures.
pub const COOLDOWN_MS: u64 = 250;
/// Default analog deadzone used for controller activity reporting.
pub const DEFAULT_DEADZONE: f32 = 0.25;

/// What the listener may recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorConfig {
    pub double_press_ms: u64,
    pub guide_enabled: bool,
    pub chord_enabled: bool,
    pub cooldown_ms: u64,
}

impl DetectorConfig {
    pub const fn new(guide_enabled: bool, chord_enabled: bool) -> Self {
        Self {
            double_press_ms: DOUBLE_PRESS_MS,
            guide_enabled,
            chord_enabled,
            cooldown_ms: COOLDOWN_MS,
        }
    }

    /// Gestures enabled by the `overlayTrigger` setting.
    ///
    /// `doubleHome` also keeps the chord as a fallback: a pad that hides its
    /// Guide button must still be able to open the overlay. `chord` disables the
    /// Guide gesture entirely for players who want Steam/OS to own that button.
    pub const fn for_trigger(trigger: TriggerChoice) -> Self {
        match trigger {
            TriggerChoice::DoubleHome => Self::new(true, true),
            TriggerChoice::Chord => Self::new(false, true),
            TriggerChoice::Keyboard => Self::new(false, false),
        }
    }
}

/// Mirror of the persisted trigger setting, kept here to avoid a dependency on
/// the settings module in the pure detector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerChoice {
    DoubleHome,
    Chord,
    Keyboard,
}

/// Stateful gesture detector. Feed it every press/release edge with a
/// monotonic timestamp; it returns [`Gesture::Toggle`] at most once per gesture.
#[derive(Debug)]
pub struct GestureDetector {
    config: DetectorConfig,
    pressed: [bool; 4],
    last_guide_press_ms: Option<u64>,
    chord_latched: bool,
    last_gesture_ms: Option<u64>,
}

impl GestureDetector {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            pressed: [false; 4],
            last_guide_press_ms: None,
            chord_latched: false,
            last_gesture_ms: None,
        }
    }

    pub fn config(&self) -> DetectorConfig {
        self.config
    }

    /// Apply a new trigger setting without dropping the pressed state, so a
    /// settings save while a button is held does not replay an edge.
    pub fn set_config(&mut self, config: DetectorConfig) {
        self.config = config;
        if !config.guide_enabled {
            self.last_guide_press_ms = None;
        }
        if !config.chord_enabled {
            self.chord_latched = false;
        }
    }

    /// Forget held buttons after a controller disconnect.
    pub fn reset(&mut self) {
        self.pressed = [false; 4];
        self.last_guide_press_ms = None;
        self.chord_latched = false;
    }

    /// One edge in, maybe one gesture out.
    pub fn handle(&mut self, event: PadEvent, now_ms: u64) -> Option<Gesture> {
        match event {
            PadEvent::Pressed(button) => {
                let index = button.index();
                // Auto-repeat: a pad that keeps reporting "pressed" for the same
                // button is not a new gesture.
                if self.pressed[index] {
                    return None;
                }
                self.pressed[index] = true;

                let mut recognised = false;

                if button == PadButton::Guide && self.config.guide_enabled {
                    match self.last_guide_press_ms {
                        Some(previous)
                            if now_ms >= previous
                                && now_ms - previous <= self.config.double_press_ms =>
                        {
                            // Consume the pair: a third press starts over
                            // instead of toggling again.
                            self.last_guide_press_ms = None;
                            recognised = true;
                        }
                        _ => self.last_guide_press_ms = Some(now_ms),
                    }
                }

                if self.config.chord_enabled
                    && !self.chord_latched
                    && self.pressed[PadButton::LeftBumper.index()]
                    && self.pressed[PadButton::RightBumper.index()]
                    && self.pressed[PadButton::Start.index()]
                {
                    self.chord_latched = true;
                    recognised = true;
                }

                if recognised {
                    self.emit(now_ms)
                } else {
                    None
                }
            }
            PadEvent::Released(button) => {
                self.pressed[button.index()] = false;
                if matches!(
                    button,
                    PadButton::LeftBumper | PadButton::RightBumper | PadButton::Start
                ) {
                    self.chord_latched = false;
                }
                None
            }
        }
    }

    fn emit(&mut self, now_ms: u64) -> Option<Gesture> {
        if let Some(previous) = self.last_gesture_ms {
            if now_ms >= previous && now_ms - previous < self.config.cooldown_ms {
                return None;
            }
        }
        self.last_gesture_ms = Some(now_ms);
        Some(Gesture::Toggle)
    }
}

/// True when an axis value is outside the deadzone. Used for the controller
/// "active" indicator in `overlay_status`, never for the gesture itself.
pub fn axis_active(value: f32, deadzone: f32) -> bool {
    let deadzone = if deadzone.is_finite() {
        deadzone.clamp(0.0, 0.9)
    } else {
        DEFAULT_DEADZONE
    };
    value.is_finite() && value.abs() > deadzone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> GestureDetector {
        GestureDetector::new(DetectorConfig::for_trigger(TriggerChoice::DoubleHome))
    }

    #[test]
    fn a_single_guide_press_is_ignored() {
        let mut detector = detector();
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_000),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::Guide), 1_080),
            None
        );
        // Well past the window: the press can never pair with a later one.
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 2_000),
            None
        );
    }

    #[test]
    fn a_double_guide_press_toggles_once() {
        let mut detector = detector();
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_000),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::Guide), 1_040),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_320),
            Some(Gesture::Toggle)
        );
        // A third press right after the pair must not toggle again.
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::Guide), 1_360),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_400),
            None
        );
    }

    #[test]
    fn two_slow_presses_are_two_singles() {
        let mut detector = detector();
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_000),
            None
        );
        detector.handle(PadEvent::Released(PadButton::Guide), 1_050);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_500),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::Guide), 1_550),
            None
        );
        // The 1_500 press already paired? It was 500 ms after the first, so it
        // only started a new window; nothing fired and nothing will now.
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 3_000),
            None
        );
    }

    #[test]
    fn repeated_pressed_events_are_debounced() {
        let mut detector = detector();
        for _ in 0..5 {
            assert_eq!(
                detector.handle(PadEvent::Pressed(PadButton::Guide), 1_000),
                None
            );
        }
        // The held button never became a release, so the second press below is
        // the first new edge; it pairs with the initial press only if inside
        // the window, which the duplicate edges must not fake.
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::Guide), 1_050),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_300),
            Some(Gesture::Toggle)
        );
    }

    #[test]
    fn the_chord_toggles_once_and_rearms_on_release() {
        let mut detector = detector();
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1_000),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::RightBumper), 1_030),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_060),
            Some(Gesture::Toggle)
        );
        // Start repeats while held: still one gesture.
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_100),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::Start), 1_200),
            None
        );
        // Pressing Start again while the bumpers are held is a new chord.
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_600),
            Some(Gesture::Toggle)
        );
    }

    #[test]
    fn the_chord_needs_all_three_buttons() {
        let mut detector = detector();
        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1_000);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 1_010);
        assert_eq!(
            detector.handle(PadEvent::Released(PadButton::RightBumper), 1_050),
            None
        );
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_100),
            None
        );
    }

    #[test]
    fn the_cooldown_swallows_a_second_gesture() {
        let mut config = DetectorConfig::for_trigger(TriggerChoice::DoubleHome);
        config.cooldown_ms = 1_000;
        let mut detector = GestureDetector::new(config);

        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1_000);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 1_010);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_020),
            Some(Gesture::Toggle)
        );
        detector.handle(PadEvent::Released(PadButton::Start), 1_100);
        detector.handle(PadEvent::Released(PadButton::LeftBumper), 1_110);
        detector.handle(PadEvent::Released(PadButton::RightBumper), 1_120);

        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1_200);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 1_210);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_220),
            None
        );
    }

    #[test]
    fn the_keyboard_trigger_disables_both_pad_gestures() {
        let mut detector =
            GestureDetector::new(DetectorConfig::for_trigger(TriggerChoice::Keyboard));
        detector.handle(PadEvent::Pressed(PadButton::Guide), 1_000);
        detector.handle(PadEvent::Released(PadButton::Guide), 1_040);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_200),
            None
        );

        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 2_000);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 2_010);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 2_020),
            None
        );
    }

    #[test]
    fn the_chord_trigger_keeps_the_guide_untouched() {
        let mut detector = GestureDetector::new(DetectorConfig::for_trigger(TriggerChoice::Chord));
        detector.handle(PadEvent::Pressed(PadButton::Guide), 1_000);
        detector.handle(PadEvent::Released(PadButton::Guide), 1_040);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Guide), 1_200),
            None
        );

        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 2_000);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 2_010);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 2_020),
            Some(Gesture::Toggle)
        );
    }

    #[test]
    fn reconnect_drops_held_buttons() {
        let mut detector = detector();
        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1_000);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 1_010);
        detector.reset();
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_100),
            None
        );
        detector.handle(PadEvent::Released(PadButton::Start), 1_150);
        detector.handle(PadEvent::Pressed(PadButton::LeftBumper), 1_200);
        detector.handle(PadEvent::Pressed(PadButton::RightBumper), 1_210);
        assert_eq!(
            detector.handle(PadEvent::Pressed(PadButton::Start), 1_220),
            Some(Gesture::Toggle)
        );
    }

    #[test]
    fn axis_deadzone_ignores_drift_and_bad_values() {
        assert!(!axis_active(0.24, DEFAULT_DEADZONE));
        assert!(axis_active(0.26, DEFAULT_DEADZONE));
        assert!(axis_active(-0.8, DEFAULT_DEADZONE));
        assert!(!axis_active(f32::NAN, DEFAULT_DEADZONE));
        // A bad deadzone falls back to the default, so a strong input is active.
        assert!(axis_active(0.5, f32::NAN));
    }
}
