//! Input devices the game will see, and the per-game preferred controller
//! (#314).
//!
//! A racing wheel is enumerated by SDL as game controller 0, so Wine/Proton
//! bind XInput device 0 to it and the real pad becomes invisible to the game.
//! Nothing below the game is broken: the order is simply wrong for the player.
//!
//! Two read-only pieces answer that:
//! - [`inventory`] lists the evdev/joydev devices (`/proc/bus/input/devices`
//!   plus `/dev/input/js*`) with a role, the vendor:product pair and the node
//!   index, so the UI can show the order the game will get;
//! - [`launch_for_preference`] turns the stored preference into the SDL launch
//!   environment: the other joysticks are ignored by vendor/product, the
//!   preferred one is named for the legacy joystick path.
//!
//! Every input is a path ([`inventory`] takes a root) so the fixtures run
//! without a real `/dev`. The launch path passes `/`; tests pass a temporary
//! folder with `proc/bus/input/devices` and empty `dev/input/jsN` files.
//!
//! Rules encoded here:
//! - a preference only ever hides *other* connected joysticks; the preferred
//!   device is never in its own ignore list, so a launch can never hide every
//!   pad;
//! - a missing or unplugged preferred device leaves the environment untouched
//!   and reports a warning, instead of hiding everything else;
//! - `0000:0000` and `ffff:ffff` are never written: SDL reads `0x0000/0x0000`
//!   as "ignore every controller" and `0xffff` as a wildcard, and a real device
//!   is never worth that risk;
//! - the preference lives in the device settings (`gameControllers`), not in
//!   the launch environment: it is a property of this machine.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// SDL2 (>= 2.0.5): comma-separated `0xVID/0xPID` game controllers to ignore.
pub const SDL_IGNORE_DEVICES: &str = "SDL_GAMECONTROLLER_IGNORE_DEVICES";
/// SDL3 spelling of the same list. Both are set so a runtime that already
/// moved to SDL3 applies the same choice; each runtime ignores the other name.
pub const SDL_GAMEPAD_IGNORE_DEVICES: &str = "SDL_GAMEPAD_IGNORE_DEVICES";
/// Legacy SDL 1.2 / js-backend hint naming the joystick device node.
pub const SDL_JOYSTICK_DEVICE: &str = "SDL_JOYSTICK_DEVICE";

/// Upper bound on the parsed device list: `/proc` never has thousands, and a
/// hand-made fixture must not allocate without bound.
const MAX_DEVICES: usize = 256;

/// What a device looks like to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DeviceRole {
    /// Racing wheel (G29, Fanatec, TMX…).
    Wheel,
    /// Game controller opened through XInput/evdev.
    Gamepad,
    /// Flight stick or other plain joystick.
    Joystick,
    /// Pointer.
    Mouse,
    /// Keyboard.
    Keyboard,
    /// Anything else (power button, video bus…).
    #[default]
    Unknown,
}

impl DeviceRole {
    /// True for a device SDL can present as a joystick/game controller.
    fn is_playable(self) -> bool {
        matches!(
            self,
            DeviceRole::Wheel | DeviceRole::Gamepad | DeviceRole::Joystick
        )
    }
}

/// One input device, as the game sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputDevice {
    pub name: String,
    /// Four lowercase hex digits, when the kernel exposes them.
    pub vendor: Option<String>,
    pub product: Option<String>,
    /// `vendor:product` (`046d:c260`). `None` when the kernel gives no ids,
    /// which also means SDL cannot match it in an ignore list.
    pub device_id: Option<String>,
    /// `js` index (`0` for `js0`).
    pub js_index: Option<u32>,
    /// `event` index (`261` for `event261`).
    pub event_index: Option<u32>,
    /// `/dev/input/jsN` on the real host.
    pub js_path: Option<String>,
    /// `/dev/input/eventN` on the real host.
    pub event_path: Option<String>,
    /// The kernel node still exists. A device can be listed by `/proc` after
    /// it was unplugged; it is then shown but never used for a launch.
    pub connected: bool,
    pub role: DeviceRole,
    /// SDL enumerates it as a joystick/game controller.
    pub is_joystick: bool,
}

impl InputDevice {
    /// Display label used by the launch log and the UI, e.g.
    /// `Microsoft X-Box 360 pad (045e:028e, js2)`.
    pub fn label(&self) -> String {
        let mut suffix = String::new();
        if let Some(id) = &self.device_id {
            suffix.push_str(id);
        }
        if let Some(index) = self.js_index {
            if !suffix.is_empty() {
                suffix.push_str(", ");
            }
            suffix.push_str(&format!("js{index}"));
        } else if let Some(index) = self.event_index {
            if !suffix.is_empty() {
                suffix.push_str(", ");
            }
            suffix.push_str(&format!("event{index}"));
        }
        if suffix.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({suffix})", self.name)
        }
    }

    /// The `0xVID/0xPID` pair SDL expects, when one can be written safely.
    fn ignore_pair(&self) -> Option<String> {
        let (vendor, product) = (self.vendor.as_deref()?, self.product.as_deref()?);
        if !safe_pair(vendor, product) {
            return None;
        }
        Some(format!("0x{vendor}/0x{product}"))
    }
}

/// The full list of devices read from one root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerInventory {
    /// False when `/proc/bus/input/devices` is absent (non-Linux host or a
    /// sandbox); `devices` is then empty and `note` explains why.
    pub available: bool,
    /// The file that was read, for support.
    pub source: String,
    /// Human sentence shown when the list is unavailable.
    pub note: Option<String>,
    /// Joysticks first (by `js` index), then the other input devices.
    pub devices: Vec<InputDevice>,
}

/// One stored per-game choice, saved in the device settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ControllerPreference {
    /// `vendor:product`, lowercase (`045e:028e`).
    pub device_id: String,
    /// Name last seen for that id; shown when the device is unplugged.
    pub name: String,
}

impl ControllerPreference {
    /// Trim and normalise the id; `None` when it is not a usable pair.
    pub fn normalized(mut self) -> Option<Self> {
        self.device_id = normalize_device_id(&self.device_id)?;
        self.name = self.name.trim().to_string();
        Some(self)
    }
}

/// What a launch would add to the environment (#314).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ControllerLaunch {
    /// Variables added to the game environment, before the per-game overrides.
    pub env: BTreeMap<String, String>,
    /// Set when the preference was applied, for the launch log.
    pub summary: Option<String>,
    /// Set when the preference could not be applied and the environment is
    /// untouched, for the launch log and the UI.
    pub warning: Option<String>,
}

impl ControllerLaunch {
    pub fn is_empty(&self) -> bool {
        self.env.is_empty()
    }
}

/// Diagnostics plus the stored preference of one game (Tauri payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameControllers {
    pub inventory: ControllerInventory,
    pub preferred: Option<ControllerPreference>,
    pub launch: ControllerLaunch,
    /// #315: host readiness plus the per-game virtual gamepad opt-in.
    pub virtual_pad: crate::virtual_pad::VirtualPadView,
}

/* ------------------------------- parsing -------------------------------- */

struct RawDevice {
    vendor: Option<String>,
    product: Option<String>,
    name: String,
    handlers: Vec<String>,
}

fn parse_proc(text: &str) -> Vec<RawDevice> {
    let mut devices = Vec::new();
    for block in text.split("\n\n") {
        let mut vendor = None;
        let mut product = None;
        let mut name = String::new();
        let mut handlers = Vec::new();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("I:") {
                for field in rest.split_whitespace() {
                    if let Some(value) = field.strip_prefix("Vendor=") {
                        vendor = Some(value.to_ascii_lowercase());
                    } else if let Some(value) = field.strip_prefix("Product=") {
                        product = Some(value.to_ascii_lowercase());
                    }
                }
            } else if let Some(rest) = line.strip_prefix("N: Name=") {
                name = unquote(rest);
            } else if let Some(rest) = line.strip_prefix("H: Handlers=") {
                handlers = rest.split_whitespace().map(str::to_string).collect();
            }
        }
        if name.is_empty() && handlers.is_empty() {
            continue;
        }
        devices.push(RawDevice {
            vendor,
            product,
            name,
            handlers,
        });
        if devices.len() >= MAX_DEVICES {
            break;
        }
    }
    devices
}

fn unquote(raw: &str) -> String {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_string()
}

fn index_of(handlers: &[String], prefix: &str) -> Option<u32> {
    handlers.iter().find_map(|handler| {
        handler
            .strip_prefix(prefix)
            .and_then(|rest| rest.parse::<u32>().ok())
    })
}

/// `4` digits of hex, never the SDL wildcard.
fn valid_component(value: &str) -> bool {
    value.len() == 4
        && value.chars().all(|character| character.is_ascii_hexdigit())
        && value != "0000"
        && value != "ffff"
}

/// A pair that can go into an SDL ignore list without hitting its wildcards.
fn safe_pair(vendor: &str, product: &str) -> bool {
    valid_component(vendor) && valid_component(product)
}

/// Normalise `vendor:product` (`046d:c260`, `046D:C260`); `None` when the
/// value is not a usable pair.
pub fn normalize_device_id(raw: &str) -> Option<String> {
    let value = raw.trim().to_ascii_lowercase();
    let (vendor, product) = value.split_once(':')?;
    if !safe_pair(vendor, product) {
        return None;
    }
    Some(format!("{vendor}:{product}"))
}

fn role_of(name: &str, handlers: &[String]) -> DeviceRole {
    let lower = name.to_ascii_lowercase();
    let has = |needle: &str| lower.contains(needle);
    let handler = |needle: &str| handlers.iter().any(|entry| entry.contains(needle));

    // Pointers and keyboards first: "touchpad" and "keypad" contain "pad".
    if handler("mouse") || has("mouse") || has("touchpad") || has("trackpad") || has("pointer") {
        return DeviceRole::Mouse;
    }
    if handler("kbd") || has("keyboard") || has("keypad") {
        return DeviceRole::Keyboard;
    }
    // Wheels before the generic "pad" rule: "Driving Force" has no pad word,
    // but "racing wheel" must never be mistaken for a controller.
    if has("wheel")
        || has("driving force")
        || has("g29")
        || has("g920")
        || has("g923")
        || has("g27")
        || has("fanatec")
        || has("t150")
        || has("t300")
        || has("t500")
        || has("tmx")
        || has("racing")
        || has("pedals")
    {
        return DeviceRole::Wheel;
    }
    if has("xbox")
        || has("x-box")
        || has("gamepad")
        || has("game pad")
        || has("pad")
        || has("controller")
        || has("dualsense")
        || has("dualshock")
        || has("joy-con")
        || has("joycon")
        || has("8bitdo")
        || has("gamesir")
        || has("steam deck")
        || has("virtual")
    {
        return DeviceRole::Gamepad;
    }
    if has("joystick") || has("flight") || has("hotas") || has("throttle") {
        return DeviceRole::Joystick;
    }
    DeviceRole::Unknown
}

fn device_from_raw(raw: RawDevice, root: &Path) -> InputDevice {
    let js_index = index_of(&raw.handlers, "js");
    let event_index = index_of(&raw.handlers, "event");
    let js_path = js_index.map(|index| root.join(format!("dev/input/js{index}")));
    let event_path = event_index.map(|index| root.join(format!("dev/input/event{index}")));
    let connected = js_path.as_deref().is_some_and(Path::exists)
        || event_path.as_deref().is_some_and(Path::exists);
    let role = role_of(&raw.name, &raw.handlers);
    let device_id = match (raw.vendor.as_deref(), raw.product.as_deref()) {
        (Some(vendor), Some(product)) if safe_pair(vendor, product) => {
            Some(format!("{vendor}:{product}"))
        }
        _ => None,
    };
    // joydev may not be loaded: a wheel/pad with an evdev node is still a
    // joystick for SDL's udev backend.
    let is_joystick = js_index.is_some() || (role.is_playable() && event_index.is_some());
    InputDevice {
        name: raw.name,
        vendor: raw.vendor,
        product: raw.product,
        device_id,
        js_index,
        event_index,
        js_path: js_path.map(|path| path.to_string_lossy().to_string()),
        event_path: event_path.map(|path| path.to_string_lossy().to_string()),
        connected,
        role,
        is_joystick,
    }
}

/// Read the input devices under `root`.
///
/// `/` is the host; a fixture folder works the same way. A missing
/// `/proc/bus/input/devices` is not an error: the list is simply unavailable
/// (non-Linux, sandbox) and the UI shows the note.
pub fn inventory(root: &Path) -> ControllerInventory {
    let proc_path: PathBuf = root.join("proc/bus/input/devices");
    let source = proc_path.to_string_lossy().to_string();
    let text = match fs::read_to_string(&proc_path) {
        Ok(text) => text,
        Err(error) => {
            let note = if error.kind() == std::io::ErrorKind::NotFound {
                "The input device list is only available on Linux (/proc/bus/input/devices is missing).".to_string()
            } else {
                format!("Could not read {source}: {error}")
            };
            return ControllerInventory {
                available: false,
                source,
                note: Some(note),
                devices: Vec::new(),
            };
        }
    };
    let mut devices: Vec<InputDevice> = parse_proc(&text)
        .into_iter()
        .map(|raw| device_from_raw(raw, root))
        .collect();
    // Stable and useful: the joysticks the game can bind, in js order, first.
    devices.sort_by(|left, right| {
        right
            .is_joystick
            .cmp(&left.is_joystick)
            .then_with(|| {
                left.js_index
                    .unwrap_or(u32::MAX)
                    .cmp(&right.js_index.unwrap_or(u32::MAX))
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    ControllerInventory {
        available: true,
        source,
        note: None,
        devices,
    }
}

/* --------------------------- launch environment -------------------------- */

/// Build the launch environment for `preference`.
///
/// The preferred device must be present *and* connected. Otherwise the
/// environment is returned untouched with a warning: a stale preference must
/// never hide the pad the player is actually holding.
pub fn launch_for_preference(
    preference: Option<&ControllerPreference>,
    devices: &[InputDevice],
) -> ControllerLaunch {
    let Some(preference) = preference else {
        return ControllerLaunch::default();
    };
    let Some(preference) = preference.clone().normalized() else {
        return ControllerLaunch {
            warning: Some(
                "The saved preferred controller is not a valid vendor:product pair; the launch environment is untouched."
                    .to_string(),
            ),
            ..ControllerLaunch::default()
        };
    };

    let joysticks: Vec<&InputDevice> = devices
        .iter()
        .filter(|device| device.is_joystick && device.connected)
        .collect();
    let preferred = joysticks
        .iter()
        .copied()
        .find(|device| device.device_id.as_deref() == Some(preference.device_id.as_str()));
    let Some(preferred) = preferred else {
        let name = if preference.name.is_empty() {
            preference.device_id.clone()
        } else {
            format!("{} ({})", preference.name, preference.device_id)
        };
        return ControllerLaunch {
            warning: Some(format!(
                "The preferred controller {name} is not connected; the launch environment is untouched and the game follows the host order ({} joystick(s) available).",
                joysticks.len()
            )),
            ..ControllerLaunch::default()
        };
    };

    let mut env = BTreeMap::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut ignored_labels: Vec<String> = Vec::new();
    let mut unmatched = 0usize;
    for other in &joysticks {
        if other.device_id.as_deref() == Some(preference.device_id.as_str()) {
            // Identical models share the pair: they stay visible together
            // instead of hiding one of them.
            continue;
        }
        match other.ignore_pair() {
            Some(pair) => {
                if !ignored.contains(&pair) {
                    ignored.push(pair);
                    ignored_labels.push(other.label());
                }
            }
            None => unmatched += 1,
        }
    }

    if !ignored.is_empty() {
        let list = ignored.join(",");
        env.insert(SDL_IGNORE_DEVICES.to_string(), list.clone());
        env.insert(SDL_GAMEPAD_IGNORE_DEVICES.to_string(), list);
    }
    if let Some(path) = &preferred.js_path {
        env.insert(SDL_JOYSTICK_DEVICE.to_string(), path.clone());
    }

    let mut summary = format!("preferred {}", preferred.label());
    if ignored_labels.is_empty() {
        summary.push_str("; no other connected joystick to hide");
    } else {
        summary.push_str(&format!("; ignoring {}", ignored_labels.join(", ")));
    }
    if unmatched > 0 {
        summary.push_str(&format!(
            "; {unmatched} other joystick(s) have no vendor:product pair and stay visible"
        ));
    }

    ControllerLaunch {
        env,
        summary: Some(summary),
        warning: None,
    }
}

/// The stored preference for one game, normalised.
pub fn preference(
    stored: &BTreeMap<String, ControllerPreference>,
    game_id: i64,
) -> Option<ControllerPreference> {
    stored
        .get(&game_id.to_string())
        .cloned()
        .and_then(ControllerPreference::normalized)
}

/// Hide every connected physical joystick so the game sees only the virtual pad
/// (#315).
///
/// Same two SDL variables as [`launch_for_preference`], but with no exception:
/// the virtual device does not exist yet when this runs, and it is created with
/// a vendor/product pair no real pad uses ([`crate::virtual_pad::VIRTUAL_VENDOR`]),
/// so it can never appear in this list. A device without a `vendor:product`
/// pair cannot be named and stays visible; the summary says so.
///
/// `SDL_JOYSTICK_DEVICE` is deliberately not set: it names a `jsN` node, and
/// the virtual pad's node does not exist before it is created. Legacy `js`
/// consumers keep the host order; that limit is documented.
pub fn launch_ignoring_all_pads(devices: &[InputDevice]) -> ControllerLaunch {
    let mut env = BTreeMap::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut ignored_labels: Vec<String> = Vec::new();
    let mut hidden = 0usize;
    let mut unmatched = 0usize;
    for device in devices
        .iter()
        .filter(|device| device.is_joystick && device.connected)
    {
        // #315: another Ludex session may already have a virtual pad with the
        // reserved pair; hiding it would leave the second game with no pad.
        if device.device_id.as_deref() == Some(crate::virtual_pad::VIRTUAL_DEVICE_ID) {
            continue;
        }
        hidden += 1;
        match device.ignore_pair() {
            Some(pair) => {
                if !ignored.contains(&pair) {
                    ignored.push(pair);
                    ignored_labels.push(device.label());
                }
            }
            None => unmatched += 1,
        }
    }
    if !ignored.is_empty() {
        let list = ignored.join(",");
        env.insert(SDL_IGNORE_DEVICES.to_string(), list.clone());
        env.insert(SDL_GAMEPAD_IGNORE_DEVICES.to_string(), list);
    }
    let mut summary =
        format!("virtual gamepad; hiding every physical joystick ({hidden} connected)");
    if !ignored_labels.is_empty() {
        summary.push_str(&format!(": {}", ignored_labels.join(", ")));
    }
    let warning = (unmatched > 0).then(|| {
        format!(
            "{unmatched} connected joystick(s) have no vendor:product pair and stay visible to the game alongside the virtual pad."
        )
    });
    ControllerLaunch {
        env,
        summary: Some(summary),
        warning,
    }
}

/// Resolve the launch environment from the device settings and the real host.
///
/// Only called by the launch path and the diagnostics command, and only reads
/// `/proc` when a preference exists: a game without one pays nothing.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
pub fn resolve_for_launch(
    stored: &BTreeMap<String, ControllerPreference>,
    game_id: i64,
) -> ControllerLaunch {
    let Some(stored) = stored.get(&game_id.to_string()) else {
        return ControllerLaunch::default();
    };
    let Some(preference) = stored.clone().normalized() else {
        return ControllerLaunch {
            warning: Some(
                "The saved preferred controller is not a valid vendor:product pair; the launch environment is untouched."
                    .to_string(),
            ),
            ..ControllerLaunch::default()
        };
    };
    let inventory = inventory(Path::new("/"));
    if !inventory.available {
        return ControllerLaunch {
            warning: Some(format!(
                "The input device list is unavailable ({}); the launch environment is untouched.",
                inventory
                    .note
                    .unwrap_or_else(|| "no device list on this system".to_string())
            )),
            ..ControllerLaunch::default()
        };
    }
    launch_for_preference(Some(&preference), &inventory.devices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    const FIXTURE: &str = r#"I: Bus=0003 Vendor=046d Product=c260 Version=0110
N: Name="Logitech G29 Driving Force Racing Wheel"
P: Phys=usb-0000:00:14.0-2/input0
H: Handlers=event8 js0
B: PROP=0
B: EV=120013

I: Bus=0003 Vendor=045e Product=028e Version=0110
N: Name="Microsoft X-Box 360 pad"
H: Handlers=event261 js2

I: Bus=0003 Vendor=046d Product=c077 Version=0110
N: Name="USB Optical Mouse"
H: Handlers=mouse0 event3

I: Bus=0003 Vendor=046d Product=c31c Version=0110
N: Name="USB Keyboard"
H: Handlers=sysrq kbd event4

I: Bus=0003 Vendor=0000 Product=0000 Version=0110
N: Name="Power Button"
H: Handlers=kbd event1
"#;

    fn fixture(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-controllers-{label}-{}",
            crate::util::now_rfc3339().replace([':', '.', '-'], "")
        ));
        fs::create_dir_all(root.join("proc/bus/input")).unwrap();
        fs::write(root.join("proc/bus/input/devices"), FIXTURE).unwrap();
        root
    }

    fn touch_js(root: &Path, index: u32) {
        fs::create_dir_all(root.join("dev/input")).unwrap();
        File::create(root.join(format!("dev/input/js{index}"))).unwrap();
    }

    fn devices(root: &Path) -> Vec<InputDevice> {
        inventory(root)
            .devices
            .into_iter()
            .filter(|device| device.is_joystick)
            .collect()
    }

    fn pad(devices: &[InputDevice]) -> &InputDevice {
        devices
            .iter()
            .find(|device| device.device_id.as_deref() == Some("045e:028e"))
            .unwrap()
    }

    fn wheel(devices: &[InputDevice]) -> &InputDevice {
        devices
            .iter()
            .find(|device| device.device_id.as_deref() == Some("046d:c260"))
            .unwrap()
    }

    fn manual_device(
        name: &str,
        device_id: Option<&str>,
        js_index: Option<u32>,
        connected: bool,
    ) -> InputDevice {
        let (vendor, product) = device_id
            .and_then(|id| id.split_once(':'))
            .map(|(vendor, product)| (Some(vendor.to_string()), Some(product.to_string())))
            .unwrap_or((None, None));
        InputDevice {
            name: name.to_string(),
            vendor,
            product,
            device_id: device_id.map(str::to_string),
            js_index,
            event_index: js_index.map(|index| index + 100),
            js_path: js_index.map(|index| format!("/dev/input/js{index}")),
            event_path: js_index.map(|index| format!("/dev/input/event{}", index + 100)),
            connected,
            role: DeviceRole::Joystick,
            is_joystick: true,
        }
    }

    #[test]
    fn a_joystick_without_ids_stays_visible_and_is_reported() {
        let devices = vec![
            manual_device("Microsoft X-Box 360 pad", Some("045e:028e"), Some(0), true),
            manual_device("Logitech G29", Some("046d:c260"), Some(1), true),
            manual_device("Mystery stick", None, Some(2), true),
        ];
        let preference = ControllerPreference {
            device_id: "045e:028e".into(),
            name: String::new(),
        };
        let launch = launch_for_preference(Some(&preference), &devices);
        assert_eq!(
            launch.env.get(SDL_IGNORE_DEVICES).map(String::as_str),
            Some("0x046d/0xc260")
        );
        let summary = launch.summary.unwrap_or_default();
        assert!(summary.contains("ignoring Logitech G29"));
        assert!(!summary.contains("ignoring Mystery stick"));
        assert!(summary.contains("1 other joystick(s) have no vendor:product pair"));
    }

    #[test]
    fn classifies_wheel_pad_mouse_and_keyboard() {
        let root = fixture("classify");
        let all = inventory(&root).devices;
        let role = |id: &str| {
            all.iter()
                .find(|device| device.device_id.as_deref() == Some(id))
                .map(|device| device.role)
        };
        assert_eq!(role("046d:c260"), Some(DeviceRole::Wheel));
        assert_eq!(role("045e:028e"), Some(DeviceRole::Gamepad));
        assert_eq!(role("046d:c077"), Some(DeviceRole::Mouse));
        assert_eq!(role("046d:c31c"), Some(DeviceRole::Keyboard));
        let power = all
            .iter()
            .find(|device| device.name == "Power Button")
            .expect("power button parsed");
        assert!(!power.is_joystick);
        assert_eq!(power.device_id, None, "0000:0000 is never a device id");
        assert_eq!(power.js_index, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inventory_marks_connected_joysticks_by_node() {
        let root = fixture("connected");
        touch_js(&root, 0);
        touch_js(&root, 2);
        let joysticks = devices(&root);
        assert_eq!(joysticks.len(), 2);
        assert!(wheel(&joysticks).connected);
        assert!(pad(&joysticks).connected);
        assert_eq!(pad(&joysticks).js_index, Some(2));
        assert_eq!(pad(&joysticks).event_index, Some(261));
        assert_eq!(
            pad(&joysticks).js_path.as_deref(),
            Some(root.join("dev/input/js2").to_string_lossy().as_ref())
        );
        // A node that was unplugged leaves the /proc entry behind.
        fs::remove_file(root.join("dev/input/js0")).unwrap();
        fs::remove_file(root.join("dev/input/js2")).unwrap();
        let joysticks = devices(&root);
        assert!(joysticks.iter().all(|device| !device.connected));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preferred_pad_ignores_only_the_other_joysticks() {
        let root = fixture("preferred");
        touch_js(&root, 0);
        touch_js(&root, 2);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "045e:028e".into(),
            name: "Microsoft X-Box 360 pad".into(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert!(launch.warning.is_none());
        assert_eq!(
            launch.env.get(SDL_IGNORE_DEVICES).map(String::as_str),
            Some("0x046d/0xc260")
        );
        // SDL3 spelling carries the same list.
        assert_eq!(
            launch.env.get(SDL_GAMEPAD_IGNORE_DEVICES),
            launch.env.get(SDL_IGNORE_DEVICES)
        );
        // The preferred device is named for legacy js consumers only.
        assert_eq!(
            launch.env.get(SDL_JOYSTICK_DEVICE).map(String::as_str),
            Some(root.join("dev/input/js2").to_string_lossy().as_ref())
        );
        // It never appears in its own ignore list.
        assert!(!launch
            .env
            .get(SDL_IGNORE_DEVICES)
            .unwrap()
            .contains("0x045e/0x028e"));
        assert!(launch
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("ignoring"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_wheel_as_preference_ignores_the_pad_instead() {
        let root = fixture("wheel-pref");
        touch_js(&root, 0);
        touch_js(&root, 2);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "046d:c260".into(),
            name: "Logitech G29".into(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert_eq!(
            launch.env.get(SDL_IGNORE_DEVICES).map(String::as_str),
            Some("0x045e/0x028e")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_preferred_device_leaves_the_environment_untouched() {
        let root = fixture("missing");
        touch_js(&root, 0);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "045e:028e".into(),
            name: "Microsoft X-Box 360 pad".into(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert!(launch.env.is_empty());
        let warning = launch.warning.expect("a warning is reported");
        assert!(warning.contains("045e:028e"));
        assert!(warning.contains("not connected"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unplugged_preferred_device_never_hides_the_other_pads() {
        let root = fixture("unplugged");
        // Both /proc entries exist but only the wheel still has a node: the
        // stale pad preference must not hide the wheel.
        touch_js(&root, 0);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "045e:028e".into(),
            name: String::new(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert!(launch.env.is_empty());
        assert!(launch.warning.is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_single_joystick_gets_no_ignore_list() {
        let root = fixture("single");
        touch_js(&root, 2);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "045e:028e".into(),
            name: String::new(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert!(!launch.env.contains_key(SDL_IGNORE_DEVICES));
        assert_eq!(
            launch.env.get(SDL_JOYSTICK_DEVICE).map(String::as_str),
            Some(root.join("dev/input/js2").to_string_lossy().as_ref())
        );
        assert!(launch.summary.unwrap_or_default().contains("no other"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_identical_models_stay_visible_together() {
        let root = std::env::temp_dir().join(format!(
            "ludex-controllers-identical-{}",
            crate::util::now_rfc3339().replace([':', '.', '-'], "")
        ));
        fs::create_dir_all(root.join("proc/bus/input")).unwrap();
        fs::write(
            root.join("proc/bus/input/devices"),
            r#"I: Bus=0003 Vendor=045e Product=028e Version=0110
N: Name="Microsoft X-Box 360 pad"
H: Handlers=event10 js0

I: Bus=0003 Vendor=045e Product=028e Version=0110
N: Name="Microsoft X-Box 360 pad"
H: Handlers=event11 js1
"#,
        )
        .unwrap();
        touch_js(&root, 0);
        touch_js(&root, 1);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "045e:028e".into(),
            name: String::new(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert!(!launch.env.contains_key(SDL_IGNORE_DEVICES));
        assert!(launch.warning.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn wildcard_pairs_are_never_written() {
        let root = fixture("wildcard");
        touch_js(&root, 0);
        let joysticks = devices(&root);
        let preference = ControllerPreference {
            device_id: "0000:0000".into(),
            name: String::new(),
        };
        let launch = launch_for_preference(Some(&preference), &joysticks);
        assert!(launch.env.is_empty());
        assert!(launch.warning.is_some());
        assert!(normalize_device_id("0000:0000").is_none());
        assert!(normalize_device_id("ffff:028e").is_none());
        assert_eq!(
            normalize_device_id("045E:028E").as_deref(),
            Some("045e:028e")
        );
        assert!(normalize_device_id("045e").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn no_preference_leaves_the_environment_untouched() {
        let root = fixture("none");
        touch_js(&root, 0);
        let launch = launch_for_preference(None, &devices(&root));
        assert!(launch.is_empty());
        assert!(launch.summary.is_none());
        assert!(launch.warning.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_launch_path_is_a_no_op_without_a_stored_preference() {
        // No /proc read either: a game without a preference pays nothing.
        let launch = resolve_for_launch(&BTreeMap::new(), 14);
        assert!(launch.is_empty());
        assert!(launch.summary.is_none());
        assert!(launch.warning.is_none());
    }

    #[test]
    fn a_missing_proc_file_reports_an_unavailable_list() {
        let root = std::env::temp_dir().join(format!(
            "ludex-controllers-absent-{}",
            crate::util::now_rfc3339().replace([':', '.', '-'], "")
        ));
        let inventory = inventory(&root);
        assert!(!inventory.available);
        assert!(inventory.devices.is_empty());
        assert!(inventory.note.unwrap_or_default().contains("Linux"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preference_lookup_normalizes_and_rejects_garbage() {
        let mut stored = BTreeMap::new();
        stored.insert(
            "42".to_string(),
            ControllerPreference {
                device_id: " 045E:028E ".into(),
                name: "  Pad  ".into(),
            },
        );
        stored.insert(
            "43".to_string(),
            ControllerPreference {
                device_id: "not-a-pair".into(),
                name: String::new(),
            },
        );
        let found = preference(&stored, 42).expect("normalised");
        assert_eq!(found.device_id, "045e:028e");
        assert_eq!(found.name, "Pad");
        assert!(preference(&stored, 43).is_none());
        assert!(preference(&stored, 7).is_none());
    }

    // #315: the virtual gamepad hides every physical pad and never names one
    // for legacy consumers, because the virtual node does not exist yet.
    #[test]
    fn the_virtual_pad_hides_every_connected_joystick() {
        let root = fixture("virtual-all");
        touch_js(&root, 0);
        touch_js(&root, 2);
        let joysticks = devices(&root);
        let launch = launch_ignoring_all_pads(&joysticks);
        assert!(launch.warning.is_none());
        let list = launch
            .env
            .get(SDL_IGNORE_DEVICES)
            .expect("an ignore list is written");
        assert_eq!(list, "0x046d/0xc260,0x045e/0x028e");
        assert_eq!(
            launch.env.get(SDL_GAMEPAD_IGNORE_DEVICES),
            launch.env.get(SDL_IGNORE_DEVICES)
        );
        assert!(!launch.env.contains_key(SDL_JOYSTICK_DEVICE));
        let summary = launch.summary.unwrap_or_default();
        assert!(summary.contains("hiding every physical joystick (2 connected)"));
        assert!(summary.contains("Logitech G29"));
        assert!(summary.contains("Microsoft X-Box 360 pad"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unplugged_or_unidentifiable_pad_is_not_hidden_silently() {
        let mut connected =
            manual_device("Microsoft X-Box 360 pad", Some("045e:028e"), Some(2), true);
        let unplugged = manual_device("Logitech G29", Some("046d:c260"), Some(0), false);
        let mystery = manual_device("Mystery stick", None, Some(1), true);
        let launch = launch_ignoring_all_pads(&[connected.clone(), unplugged, mystery]);
        assert_eq!(
            launch.env.get(SDL_IGNORE_DEVICES).map(String::as_str),
            Some("0x045e/0x028e")
        );
        assert!(launch
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("1 connected joystick(s) have no vendor:product pair"));
        assert!(launch
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("(2 connected)"));
        // Mutating the fixture device after the call changes nothing: the
        // environment is built from the snapshot it received.
        connected.connected = false;
        assert_eq!(
            launch.env.get(SDL_IGNORE_DEVICES).map(String::as_str),
            Some("0x045e/0x028e")
        );
    }

    #[test]
    fn another_ludex_virtual_pad_is_never_hidden() {
        // A second Ludex session may already present the reserved pair; hiding
        // it would leave that session's game with no pad at all.
        let devices = vec![
            manual_device("Ludex Virtual Gamepad", Some("4c44:0001"), Some(4), true),
            manual_device("Microsoft X-Box 360 pad", Some("045e:028e"), Some(2), true),
        ];
        let launch = launch_ignoring_all_pads(&devices);
        assert_eq!(
            launch.env.get(SDL_IGNORE_DEVICES).map(String::as_str),
            Some("0x045e/0x028e")
        );
        assert!(launch.summary.unwrap_or_default().contains("(1 connected)"));
    }
}
