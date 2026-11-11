//! Controller profiles (#51): pure mapping maths shared by the web UI, the
//! desktop shell and the server-side profile validator.
//!
//! A profile describes how a physical gamepad is translated into the logical
//! actions the interface understands (`confirm`, `left`, `menu`, …):
//!
//! - stick deadzone, sensitivity and response curve;
//! - axis inversion;
//! - button and axis remapping (physical `button0`..`button31`,
//!   `axis0-`…`axis7+`);
//! - analog trigger threshold;
//! - vibration.
//!
//! Profiles are stored per user and optionally per game. Resolution is
//! deliberately simple: a game profile wins over the account profile, which
//! wins over the `xbox` preset. Nothing here depends on the browser: the same
//! rules are re-implemented in `apps/web/src/lib/controller-profiles.ts` and
//! the two implementations are kept in sync by tests on both sides.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Logical actions understood by the interface.
pub const ACTIONS: &[&str] = &[
    "left",
    "right",
    "up",
    "down",
    "confirm",
    "back",
    "menu",
    "downloads",
    "prevTab",
    "nextTab",
];

/// Built-in preset identifiers.
pub const PRESETS: &[&str] = &["xbox", "dualsense", "steamdeck", "generic"];

/// The largest button index a profile may address.
pub const MAX_BUTTON: u8 = 31;

/// The largest axis index a profile may address.
pub const MAX_AXIS: u8 = 7;

/// One controller profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControllerProfile {
    /// Display name, at most 64 characters.
    #[serde(default = "default_name")]
    pub name: String,
    /// Radial deadzone applied before anything else, `0.0..=0.95`.
    #[serde(default = "default_deadzone")]
    pub deadzone: f32,
    /// Multiplier applied after the curve, `0.1..=3.0`.
    #[serde(default = "default_sensitivity")]
    pub sensitivity: f32,
    /// Response curve exponent, `0.5..=4.0` (1.0 is linear).
    #[serde(default = "default_curve")]
    pub curve: f32,
    /// Invert the horizontal stick axis.
    #[serde(default)]
    pub invert_x: bool,
    /// Invert the vertical stick axis.
    #[serde(default)]
    pub invert_y: bool,
    /// Analog buttons at or above this value count as pressed, `0.0..=1.0`.
    #[serde(default = "default_trigger")]
    pub trigger_threshold: f32,
    /// Ask the browser/OS to rumble on actions.
    #[serde(default = "default_vibration")]
    pub vibration: bool,
    /// Physical input to logical action. Keys are `buttonN`, `axisN-` or
    /// `axisN+`; values are one of [`ACTIONS`].
    #[serde(default)]
    pub mappings: BTreeMap<String, String>,
}

fn default_name() -> String {
    "Custom".to_string()
}

const fn default_deadzone() -> f32 {
    0.18
}

const fn default_sensitivity() -> f32 {
    1.0
}

const fn default_curve() -> f32 {
    1.0
}

const fn default_trigger() -> f32 {
    0.35
}

const fn default_vibration() -> bool {
    true
}

impl Default for ControllerProfile {
    fn default() -> Self {
        preset("xbox")
    }
}

impl ControllerProfile {
    /// Builds a profile from a built-in preset. Unknown names fall back to
    /// `xbox`.
    #[must_use]
    pub fn from_preset(name: &str) -> Self {
        preset(name)
    }

    /// Validates the profile and returns a human-readable reason on failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("profile name is required".to_string());
        }
        if self.name.chars().count() > 64 {
            return Err("profile name must be at most 64 characters".to_string());
        }
        if self.name.chars().any(char::is_control) {
            return Err("profile name must not contain control characters".to_string());
        }
        if !(0.0..=0.95).contains(&self.deadzone) {
            return Err("deadzone must be between 0 and 0.95".to_string());
        }
        if !(0.1..=3.0).contains(&self.sensitivity) {
            return Err("sensitivity must be between 0.1 and 3".to_string());
        }
        if !(0.5..=4.0).contains(&self.curve) {
            return Err("curve must be between 0.5 and 4".to_string());
        }
        if !(0.0..=1.0).contains(&self.trigger_threshold) {
            return Err("triggerThreshold must be between 0 and 1".to_string());
        }
        if self.mappings.len() > 64 {
            return Err("a profile may map at most 64 inputs".to_string());
        }
        for (input, action) in &self.mappings {
            validate_input(input)?;
            if !ACTIONS.contains(&action.as_str()) {
                return Err(format!("unknown action `{action}`"));
            }
        }
        Ok(())
    }

    /// Resolves the logical action bound to a button, honouring the analog
    /// trigger threshold. A missing mapping means the button is inert.
    #[must_use]
    pub fn action_for_button(&self, button: u8, value: f32) -> Option<&str> {
        let threshold = if button == 6 || button == 7 {
            self.trigger_threshold
        } else {
            0.5
        };
        if value < threshold {
            return None;
        }
        self.mappings
            .get(&format!("button{button}"))
            .map(String::as_str)
    }

    /// Resolves the logical action for a stick reading. `axis` selects the
    /// physical stick (0 = left, 1 = right); the raw `x`/`y` values come from
    /// the Gamepad API and are corrected before thresholding.
    #[must_use]
    pub fn action_for_axis(&self, axis: u8, x: f32, y: f32) -> Option<&str> {
        if axis > MAX_AXIS {
            return None;
        }
        let (x, y) = (
            self.apply_axis(axis, x, y, true),
            self.apply_axis(axis, x, y, false),
        );
        // Dominant direction wins; ties prefer horizontal movement.
        if x.abs() >= y.abs() {
            if x <= -0.5 {
                return self
                    .mappings
                    .get(&format!("axis{axis}-"))
                    .map(String::as_str);
            }
            if x >= 0.5 {
                return self
                    .mappings
                    .get(&format!("axis{axis}+"))
                    .map(String::as_str);
            }
        } else {
            if y <= -0.5 {
                return self
                    .mappings
                    .get(&format!("axis{axis}-"))
                    .map(String::as_str);
            }
            if y >= 0.5 {
                return self
                    .mappings
                    .get(&format!("axis{axis}+"))
                    .map(String::as_str);
            }
        }
        None
    }

    /// Corrected value for one stick component after deadzone, curve,
    /// sensitivity and inversion.
    #[must_use]
    pub fn apply_axis(&self, axis: u8, x: f32, y: f32, horizontal: bool) -> f32 {
        let raw = if horizontal { x } else { y };
        let inverted = if horizontal {
            self.invert_x
        } else {
            self.invert_y
        };
        let _ = axis;
        correct_axis(raw, self.deadzone, self.curve, self.sensitivity, inverted)
    }

    /// Serialises the profile to the import/export JSON document.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parses and validates an imported profile document.
    pub fn from_json(document: &str) -> Result<Self, String> {
        let profile: Self = serde_json::from_str(document).map_err(|error| error.to_string())?;
        profile.validate()?;
        Ok(profile)
    }
}

/// Corrects one raw axis reading.
#[must_use]
pub fn correct_axis(raw: f32, deadzone: f32, curve: f32, sensitivity: f32, inverted: bool) -> f32 {
    let value = raw.clamp(-1.0, 1.0);
    let magnitude = value.abs();
    if magnitude <= deadzone {
        return 0.0;
    }
    let normalised = ((magnitude - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    let shaped = normalised.powf(curve);
    let scaled = (shaped * sensitivity).clamp(0.0, 1.0);
    let signed = if value.is_sign_negative() {
        -scaled
    } else {
        scaled
    };
    if inverted {
        -signed
    } else {
        signed
    }
}

/// Resolves the effective profile for a game.
///
/// A per-game profile wins as a whole; otherwise the account profile is used;
/// otherwise the `xbox` preset. The server stores exactly the same document,
/// so the resolution cannot drift between the two sides.
#[must_use]
pub fn resolve<'a>(
    global: Option<&'a ControllerProfile>,
    game: Option<&'a ControllerProfile>,
) -> ControllerProfile {
    game.or(global).cloned().unwrap_or_else(|| preset("xbox"))
}

/// Builds a built-in preset.
#[must_use]
pub fn preset(name: &str) -> ControllerProfile {
    match name.trim().to_ascii_lowercase().as_str() {
        "dualsense" | "playstation" | "ps5" => ControllerProfile {
            name: "DualSense".to_string(),
            trigger_threshold: 0.25,
            ..base_profile("DualSense")
        },
        "steamdeck" | "steam-deck" | "deck" => {
            let mut profile = base_profile("Steam Deck");
            // Back buttons (L4/R4) reach the browser as 16/17 on the Deck's
            // standard-mapping mode; bind them to the two quick actions.
            profile
                .mappings
                .insert("button16".to_string(), "menu".to_string());
            profile
                .mappings
                .insert("button17".to_string(), "downloads".to_string());
            profile
        }
        "generic" | "standard" => {
            let mut profile = base_profile("Generic");
            profile.vibration = false;
            profile
        }
        _ => base_profile("Xbox"),
    }
}

fn base_profile(name: &str) -> ControllerProfile {
    let mut mappings = BTreeMap::new();
    let standard = [
        ("button0", "confirm"),
        ("button1", "back"),
        ("button2", "menu"),
        ("button3", "downloads"),
        ("button4", "prevTab"),
        ("button5", "nextTab"),
        ("button12", "up"),
        ("button13", "down"),
        ("button14", "left"),
        ("button15", "right"),
        ("axis0-", "left"),
        ("axis0+", "right"),
        ("axis1-", "up"),
        ("axis1+", "down"),
    ];
    for (input, action) in standard {
        mappings.insert(input.to_string(), action.to_string());
    }
    ControllerProfile {
        name: name.to_string(),
        deadzone: default_deadzone(),
        sensitivity: default_sensitivity(),
        curve: default_curve(),
        invert_x: false,
        invert_y: false,
        trigger_threshold: default_trigger(),
        vibration: true,
        mappings,
    }
}

fn validate_input(input: &str) -> Result<(), String> {
    if let Some(rest) = input.strip_prefix("button") {
        let index: u8 = rest
            .parse()
            .map_err(|_| format!("invalid button input `{input}`"))?;
        if index > MAX_BUTTON {
            return Err(format!("button index out of range in `{input}`"));
        }
        return Ok(());
    }
    if let Some(rest) = input.strip_prefix("axis") {
        let (index, direction) = rest
            .split_at_checked(rest.len().saturating_sub(1))
            .ok_or_else(|| format!("invalid axis input `{input}`"))?;
        let index: u8 = index
            .parse()
            .map_err(|_| format!("invalid axis input `{input}`"))?;
        if index > MAX_AXIS || !matches!(direction, "-" | "+") {
            return Err(format!("invalid axis input `{input}`"));
        }
        return Ok(());
    }
    Err(format!("invalid input `{input}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_zeros_the_centre_and_rescales_the_rest() {
        assert_eq!(correct_axis(0.1, 0.2, 1.0, 1.0, false), 0.0);
        assert_eq!(correct_axis(-0.2, 0.2, 1.0, 1.0, false), 0.0);
        // (0.6 - 0.2) / 0.8 = 0.5
        assert!((correct_axis(0.6, 0.2, 1.0, 1.0, false) - 0.5).abs() < 1e-6);
        assert!((correct_axis(-0.6, 0.2, 1.0, 1.0, false) + 0.5).abs() < 1e-6);
    }

    #[test]
    fn sensitivity_and_curve_shape_the_response() {
        let linear = correct_axis(0.6, 0.1, 1.0, 1.0, false);
        let boosted = correct_axis(0.6, 0.1, 1.0, 1.5, false);
        assert!(boosted > linear);
        let aggressive = correct_axis(0.6, 0.1, 0.5, 1.0, false);
        assert!(aggressive > linear);
        let soft = correct_axis(0.6, 0.1, 2.0, 1.0, false);
        assert!(soft < linear);
        assert!(correct_axis(1.0, 0.0, 1.0, 3.0, false) <= 1.0);
    }

    #[test]
    fn inversion_flips_the_sign() {
        assert!(correct_axis(0.7, 0.0, 1.0, 1.0, true) < 0.0);
        assert!(correct_axis(-0.7, 0.0, 1.0, 1.0, true) > 0.0);
    }

    #[test]
    fn presets_round_trip_and_resolve() {
        for name in PRESETS {
            let profile = preset(name);
            profile.validate().expect("preset is valid");
            let json = profile.to_json().expect("serialise preset");
            let parsed = ControllerProfile::from_json(&json).expect("parse preset");
            assert_eq!(profile, parsed);
        }
        let global = preset("dualsense");
        let game = preset("steamdeck");
        assert_eq!(resolve(Some(&global), None).name, "DualSense");
        assert_eq!(resolve(Some(&global), Some(&game)).name, "Steam Deck");
        assert_eq!(resolve(None, None).name, "Xbox");
    }

    #[test]
    fn button_and_axis_mapping_use_the_profile() {
        let mut profile = preset("xbox");
        profile
            .mappings
            .insert("button0".to_string(), "menu".to_string());
        assert_eq!(profile.action_for_button(0, 1.0), Some("menu"));
        assert_eq!(profile.action_for_button(0, 0.2), None);
        // Triggers use the analog threshold from the profile.
        assert_eq!(profile.action_for_button(6, 0.3), None);
        profile
            .mappings
            .insert("button6".to_string(), "downloads".to_string());
        assert_eq!(profile.action_for_button(6, 0.3), None);
        assert_eq!(profile.action_for_button(6, 0.9), Some("downloads"));
        assert_eq!(profile.action_for_axis(0, 0.9, 0.0), Some("right"));
        assert_eq!(profile.action_for_axis(0, -0.9, 0.0), Some("left"));
        assert_eq!(profile.action_for_axis(1, 0.0, -0.9), Some("up"));
        assert_eq!(profile.action_for_axis(1, 0.0, 0.1), None);
    }

    #[test]
    fn validation_rejects_bad_values() {
        let mut profile = preset("xbox");
        profile.deadzone = 1.5;
        assert!(profile.validate().is_err());
        let mut profile = preset("xbox");
        profile
            .mappings
            .insert("button0".to_string(), "teleport".to_string());
        assert!(profile.validate().is_err());
        let mut profile = preset("xbox");
        profile
            .mappings
            .insert("button99".to_string(), "confirm".to_string());
        assert!(profile.validate().is_err());
        assert!(ControllerProfile::from_json("{}").is_ok());
        assert!(ControllerProfile::from_json("{\"deadzone\":-1}").is_err());
    }
}
