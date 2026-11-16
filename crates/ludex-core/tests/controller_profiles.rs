//! Shared controller-profile fixture (#51, #237).
//!
//! `apps/web/src/lib/controller-profiles.ts` re-implements the mapping maths
//! from `ludex_core::gamepad`. Both suites now assert the same JSON document,
//! so a change on one side cannot silently drift from the other.

use ludex_core::gamepad::ControllerProfile;

const FIXTURE: &str = include_str!("fixtures/controller-profiles.json");

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AxisCase {
    name: String,
    x: f32,
    expected: f32,
    #[serde(default)]
    tolerance: f32,
}

#[derive(Debug, serde::Deserialize)]
struct ButtonCase {
    name: String,
    button: u8,
    value: f32,
    action: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct AxisMappingCase {
    name: String,
    axis: u8,
    x: f32,
    y: f32,
    action: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    profile: ControllerProfile,
    axis_cases: Vec<AxisCase>,
    button_cases: Vec<ButtonCase>,
    axis_mapping_cases: Vec<AxisMappingCase>,
}

fn fixture() -> Fixture {
    serde_json::from_str(FIXTURE).expect("controller profile fixture")
}

#[test]
fn profile_is_valid_on_the_rust_side() {
    fixture()
        .profile
        .validate()
        .expect("fixture profile is valid");
}

#[test]
fn axis_cases_match() {
    let fixture = fixture();
    for case in fixture.axis_cases {
        let actual = fixture.profile.apply_axis(0, case.x, 0.0, true);
        assert!(
            (actual - case.expected).abs() <= case.tolerance,
            "{}: expected {}, got {actual}",
            case.name,
            case.expected
        );
    }
}

#[test]
fn button_cases_match() {
    let fixture = fixture();
    for case in fixture.button_cases {
        assert_eq!(
            fixture
                .profile
                .action_for_button(case.button, case.value)
                .map(str::to_string),
            case.action,
            "{}",
            case.name
        );
    }
}

#[test]
fn axis_mapping_cases_match() {
    let fixture = fixture();
    for case in fixture.axis_mapping_cases {
        assert_eq!(
            fixture
                .profile
                .action_for_axis(case.axis, case.x, case.y)
                .map(str::to_string),
            case.action,
            "{}",
            case.name
        );
    }
}
