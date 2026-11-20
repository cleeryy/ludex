//! One virtual gamepad Ludex presents to the game (#315).
//!
//! Ludex already reads pads for its own overlay and Console mode, and #314 can
//! hide the *other* joysticks with SDL's ignore list. Neither changes what the
//! game binds when the first joystick is a wheel. This module adds the missing
//! half: a virtual pad the game can bind, created through Linux `uinput`, fed
//! by the physical pad the player selected.
//!
//! The shape of a launch with the option on:
//!
//! 1. Ludex creates `/dev/uinput` device `Ludex Virtual Gamepad` with the
//!    standard Xbox-style capability set (10 buttons + guide, four stick axes,
//!    two triggers, one hat);
//! 2. it opens the source pad's `/dev/input/eventN` node and copies button,
//!    axis and hat events onto the virtual device, with the source's axis
//!    ranges rescaled to the descriptor's ranges — no deadzone, no curve, so
//!    the game sees exactly the values the pad produced;
//! 3. the launch environment hides every connected physical joystick from SDL
//!    ([`crate::controllers::launch_ignoring_all_pads`]), so the game sees one
//!    controller: the virtual one;
//! 4. the bridge stops when the game session ends and `UI_DEV_DESTROY` removes
//!    the device.
//!
//! Choices made here, and why:
//!
//! - **`uinput`, not `uhid`.** A `uinput` evdev pad is what SDL and Wine's
//!   winebus backend read on Linux; a synthetic HID descriptor through `uhid`
//!   would present a real Xbox 360 device to XInput, which is better under
//!   Proton but needs a hand-written HID report descriptor. This pass ships the
//!   honest evdev one and documents the XInput limitation
//!   (`docs/CONTROLLERS.md`); `uhid` remains the follow-up.
//! - **The source device is never grabbed** (`EVIOCGRAB` is not called). The
//!   overlay and Console mode keep reading it through `gilrs`, and a crash in
//!   the bridge can never leave the pad dead until replug. The cost is
//!   documented: a native game that opens evdev directly (not through SDL)
//!   still sees the physical pad.
//! - **No new heavy dependency.** The ioctl numbers, `struct uinput_setup` and
//!   `struct uinput_abs_setup` are written by hand from
//!   `include/uapi/linux/uinput.h`; only `libc` (already in the lock graph
//!   through tokio and gilrs) is used for `open`/`ioctl`/`poll`/`read`.
//! - **Injected traits.** [`UinputBackend`] and [`EvdevSource`] keep the whole
//!   decision surface testable without `/dev/uinput` or a real pad, which is
//!   why the tests below never touch hardware.
//! - **Missing permissions are not a launch failure.** A read-only `/dev/uinput`
//!   or a missing module leaves the physical pads untouched and logs a warning;
//!   the game still starts.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::controllers::{
    ControllerInventory, ControllerLaunch, ControllerPreference, InputDevice,
};

/// Name the game (and `evtest`) shows for the virtual device.
pub const VIRTUAL_PAD_NAME: &str = "Ludex Virtual Gamepad";
/// Fake vendor/product: no real pad uses the "LD" vendor id, so the ignore list
/// built for the physical devices can never match the virtual one.
pub const VIRTUAL_VENDOR: u16 = 0x4c44;
pub const VIRTUAL_PRODUCT: u16 = 0x0001;
pub const VIRTUAL_VERSION: u16 = 0x0001;
/// The `vendor:product` pair reserved for the virtual device. A launch that
/// hides the physical pads must never write it: another Ludex session may have
/// created a virtual pad with the same pair.
pub const VIRTUAL_DEVICE_ID: &str = "4c44:0001";
/// `BUS_USB`: what a real wired pad reports and what SDL's udev backend expects.
pub const VIRTUAL_BUSTYPE: u16 = 0x0003;
/// The kernel entry point. Exists only when the `uinput` module is loaded and
/// the udev rule from `docs/CONTROLLERS.md` grants access.
pub const UINPUT_PATH: &str = "/dev/uinput";
/// How long one read waits before the loop re-checks the stop flag.
const POLL_MS: i32 = 100;

/* --------------------------- Linux input codes --------------------------- */

/// `include/uapi/linux/input-event-codes.h`, spelled out so the module does not
/// depend on a bindgen crate for thirteen numbers.
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_ABS: u16 = 0x03;
pub const SYN_REPORT: u16 = 0x00;

pub const BTN_SOUTH: u16 = 0x130; // A
pub const BTN_EAST: u16 = 0x131; // B
pub const BTN_NORTH: u16 = 0x133; // Y
pub const BTN_WEST: u16 = 0x134; // X
pub const BTN_TL: u16 = 0x136;
pub const BTN_TR: u16 = 0x137;
pub const BTN_SELECT: u16 = 0x13a;
pub const BTN_START: u16 = 0x13b;
pub const BTN_MODE: u16 = 0x13c;
pub const BTN_THUMBL: u16 = 0x13d;
pub const BTN_THUMBR: u16 = 0x13e;

pub const BTN_DPAD_UP: u16 = 0x220;
pub const BTN_DPAD_DOWN: u16 = 0x221;
pub const BTN_DPAD_LEFT: u16 = 0x222;
pub const BTN_DPAD_RIGHT: u16 = 0x223;

pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_RY: u16 = 0x04;
pub const ABS_RZ: u16 = 0x05;
pub const ABS_HAT0X: u16 = 0x10;
pub const ABS_HAT0Y: u16 = 0x11;

/// The axes the bridge reads from the source device and writes to the virtual
/// one. Kept separate from [`descriptor`] so the evdev side can ask for ranges
/// before the device is created.
const AXIS_CODES: [u16; 8] = [
    ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y,
];

/* ------------------------------ descriptor ------------------------------- */

/// One absolute axis on the virtual device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisSpec {
    pub code: u16,
    pub min: i32,
    pub max: i32,
    /// Kernel-side jitter filter. Non-zero only for the sticks, like a real pad.
    pub fuzz: i32,
    pub flat: i32,
}

/// What the virtual device advertises. Built once per launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadDescriptor {
    pub name: String,
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    pub buttons: Vec<u16>,
    pub axes: Vec<AxisSpec>,
}

/// The capability set: Xbox 360 shape, `32767` sticks and `0..255` triggers.
///
/// A hat is used for the d-pad instead of `BTN_DPAD_*`: SDL maps an Xbox pad's
/// hat to the d-pad, and the source's own d-pad buttons are translated to it,
/// so both pad generations end up with a working d-pad.
pub fn descriptor() -> PadDescriptor {
    PadDescriptor {
        name: VIRTUAL_PAD_NAME.to_string(),
        bustype: VIRTUAL_BUSTYPE,
        vendor: VIRTUAL_VENDOR,
        product: VIRTUAL_PRODUCT,
        version: VIRTUAL_VERSION,
        buttons: vec![
            BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START,
            BTN_MODE, BTN_THUMBL, BTN_THUMBR,
        ],
        axes: vec![
            AxisSpec {
                code: ABS_X,
                min: -32768,
                max: 32767,
                fuzz: 16,
                flat: 128,
            },
            AxisSpec {
                code: ABS_Y,
                min: -32768,
                max: 32767,
                fuzz: 16,
                flat: 128,
            },
            AxisSpec {
                code: ABS_RX,
                min: -32768,
                max: 32767,
                fuzz: 16,
                flat: 128,
            },
            AxisSpec {
                code: ABS_RY,
                min: -32768,
                max: 32767,
                fuzz: 16,
                flat: 128,
            },
            AxisSpec {
                code: ABS_Z,
                min: 0,
                max: 255,
                fuzz: 0,
                flat: 0,
            },
            AxisSpec {
                code: ABS_RZ,
                min: 0,
                max: 255,
                fuzz: 0,
                flat: 0,
            },
            AxisSpec {
                code: ABS_HAT0X,
                min: -1,
                max: 1,
                fuzz: 0,
                flat: 0,
            },
            AxisSpec {
                code: ABS_HAT0Y,
                min: -1,
                max: 1,
                fuzz: 0,
                flat: 0,
            },
        ],
    }
}

/* -------------------------------- events --------------------------------- */

/// One raw evdev event read from the source pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

/// One event written to the virtual device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadEvent {
    Key {
        code: u16,
        value: i32,
    },
    Abs {
        code: u16,
        value: i32,
    },
    /// `SYN_REPORT`: without it the kernel buffers the events and the game sees
    /// nothing until the buffer fills.
    Syn,
}

/// Source axis range, as `EVIOCGABS` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub min: i32,
    pub max: i32,
}

/// Copy one source event onto the virtual pad.
///
/// - keys the descriptor advertises pass through unchanged;
/// - `BTN_DPAD_*` become hat values, because the descriptor has no d-pad buttons;
/// - absolute axes are rescaled from the source's range to the descriptor's,
///   and clamped when the source reports no range at all. No deadzone is
///   applied: the game gets what the pad produced;
/// - `SYN_REPORT` is forwarded, everything else (auto-repeat, MSC, FF) is
///   dropped.
pub fn translate(
    event: SourceEvent,
    ranges: &BTreeMap<u16, SourceRange>,
    descriptor: &PadDescriptor,
) -> Vec<PadEvent> {
    match event.kind {
        EV_KEY => {
            if descriptor.buttons.contains(&event.code) {
                return vec![PadEvent::Key {
                    code: event.code,
                    value: event.value,
                }];
            }
            let hat = match event.code {
                BTN_DPAD_UP => (ABS_HAT0Y, -1),
                BTN_DPAD_DOWN => (ABS_HAT0Y, 1),
                BTN_DPAD_LEFT => (ABS_HAT0X, -1),
                BTN_DPAD_RIGHT => (ABS_HAT0X, 1),
                _ => return Vec::new(),
            };
            vec![PadEvent::Abs {
                code: hat.0,
                value: hat.1 * event.value.clamp(0, 1),
            }]
        }
        EV_ABS => {
            let Some(axis) = descriptor.axes.iter().find(|axis| axis.code == event.code) else {
                return Vec::new();
            };
            let value = match ranges.get(&event.code) {
                Some(range) => rescale(*range, event.value, axis.min, axis.max),
                None => event.value.clamp(axis.min, axis.max),
            };
            vec![PadEvent::Abs {
                code: event.code,
                value,
            }]
        }
        EV_SYN if event.code == SYN_REPORT => vec![PadEvent::Syn],
        _ => Vec::new(),
    }
}

/// Linear rescale; `i64` because a full `i32` span overflows `i32` in the
/// intermediate product.
fn rescale(range: SourceRange, value: i32, min: i32, max: i32) -> i32 {
    if range.max <= range.min || max <= min {
        return value.clamp(min, max);
    }
    let source = i64::from(value.clamp(range.min, range.max)) - i64::from(range.min);
    let span = i64::from(range.max) - i64::from(range.min);
    let target = i64::from(max) - i64::from(min);
    (i64::from(min) + source * target / span) as i32
}

/* -------------------------------- backends ------------------------------- */

/// The virtual-device side. Implemented by [`uinput::UinputDevice`] on Linux and
/// by a recorder in the tests.
pub trait UinputBackend: Send + 'static {
    /// Register the capabilities and call `UI_DEV_CREATE`.
    fn create(&mut self, descriptor: &PadDescriptor) -> Result<(), String>;
    fn emit(&mut self, event: PadEvent) -> Result<(), String>;
    /// `UI_DEV_DESTROY`, then close. Called once by the bridge thread, or by
    /// `Drop` when the device never reached the thread.
    fn destroy(&mut self);
}

/// The physical side. Implemented by [`evdev::EvdevDevice`] on Linux and by a
/// scripted source in the tests.
pub trait EvdevSource: Send + 'static {
    /// `EVIOCGABS` for every axis in [`AXIS_CODES`]; missing entries mean "the
    /// pad does not expose this axis".
    fn axis_ranges(&mut self) -> BTreeMap<u16, SourceRange>;
    /// Wait up to `timeout_ms` for a batch of raw events. An empty batch is a
    /// timeout, not an error.
    fn read(&mut self, timeout_ms: i32) -> Result<Vec<SourceEvent>, String>;
}

/// The running bridge. Dropping it stops the reader thread and destroys the
/// device.
pub struct VirtualPadBridge {
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl VirtualPadBridge {
    /// Create the virtual device, then start copying events. The device exists
    /// before this returns, so the game spawned right after enumerates it.
    pub fn start(
        mut backend: Box<dyn UinputBackend>,
        mut source: Box<dyn EvdevSource>,
        descriptor: PadDescriptor,
    ) -> Result<Self, String> {
        backend.create(&descriptor)?;
        let ranges = source.axis_ranges();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("ludex-virtual-pad".to_string())
            .spawn(move || {
                'running: while !flag.load(Ordering::Relaxed) {
                    let events = match source.read(POLL_MS) {
                        Ok(events) => events,
                        Err(error) => {
                            // The pad was unplugged or its node went away: the
                            // virtual device stays but carries no input. Stop
                            // instead of spinning on a dead fd.
                            tracing::warn!(error = %error, "the virtual gamepad source stopped");
                            break;
                        }
                    };
                    for event in events {
                        for pad_event in translate(event, &ranges, &descriptor) {
                            if let Err(error) = backend.emit(pad_event) {
                                tracing::warn!(
                                    error = %error,
                                    "the virtual gamepad stopped accepting events"
                                );
                                break 'running;
                            }
                        }
                    }
                }
                backend.destroy();
            })
            .map_err(|error| format!("Could not start the virtual gamepad thread: {error}"))?;
        Ok(Self {
            stop,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Stop reading and destroy the device. Idempotent: the second call is a
    /// no-op, which is what the session teardown and `stop_game` rely on.
    pub fn stop(&self) {
        let handle = match self.thread.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(handle) = handle {
            self.stop.store(true, Ordering::SeqCst);
            let _ = handle.join();
        }
    }

    /// True while the reader thread owns the device. Test-only observability:
    /// the app never needs to ask, it stops the bridge and moves on.
    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        match self.thread.lock() {
            Ok(slot) => slot.is_some(),
            Err(poisoned) => poisoned.into_inner().is_some(),
        }
    }
}

impl Drop for VirtualPadBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

/* ----------------------------- availability ------------------------------ */

/// What the host can do, surfaced in Settings → Controllers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VirtualPadStatus {
    Available,
    /// `/dev/uinput` is missing, or present and not writable by this user.
    MissingPermission,
    /// macOS, or Windows without ViGEm yet.
    UnsupportedPlatform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualPadAvailability {
    pub status: VirtualPadStatus,
    pub path: String,
    /// One sentence the UI shows verbatim, with the fix when there is one.
    pub detail: String,
}

/// Probe the host. Read-only: the check is `access(W_OK)`, never a write.
pub fn availability() -> VirtualPadAvailability {
    #[cfg(target_os = "linux")]
    {
        let path = Path::new(UINPUT_PATH);
        if !path.exists() {
            return VirtualPadAvailability {
                status: VirtualPadStatus::MissingPermission,
                path: UINPUT_PATH.to_string(),
                detail: format!(
                    "{UINPUT_PATH} is missing. Load the module (sudo modprobe uinput) and add the udev rule from docs/CONTROLLERS.md, then restart Ludex."
                ),
            };
        }
        if !writable(path) {
            return VirtualPadAvailability {
                status: VirtualPadStatus::MissingPermission,
                path: UINPUT_PATH.to_string(),
                detail: format!(
                    "{UINPUT_PATH} exists but this account cannot write to it. Add the account to the uinput group and apply the udev rule from docs/CONTROLLERS.md."
                ),
            };
        }
        VirtualPadAvailability {
            status: VirtualPadStatus::Available,
            path: UINPUT_PATH.to_string(),
            detail: format!("{UINPUT_PATH} is writable; Ludex can create {VIRTUAL_PAD_NAME}."),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        VirtualPadAvailability {
            status: VirtualPadStatus::UnsupportedPlatform,
            path: String::new(),
            detail: "The virtual gamepad is Linux-only for now; Windows needs ViGEmBus and macOS has no supported backend.".to_string(),
        }
    }
}

#[cfg(target_os = "linux")]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(raw) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // access(2): a probe, not an open, so a device node is never held open.
    unsafe { libc::access(raw.as_ptr(), libc::W_OK) == 0 }
}

/* ------------------------------- per game -------------------------------- */

/// The stored per-game opt-in (`settings.json`, `gameVirtualPads`). Device-local
/// like the preferred controller: it names hardware this machine has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct VirtualPadPreference {
    pub enabled: bool,
    /// Optional `vendor:product` override; empty means "the game's preferred
    /// controller, else the first pad" (see [`source_device`]).
    pub device_id: String,
}

impl VirtualPadPreference {
    /// `None` when the option is off: a disabled entry is never stored.
    pub fn normalized(mut self) -> Option<Self> {
        if !self.enabled {
            return None;
        }
        self.device_id = self.device_id.trim().to_ascii_lowercase();
        if !self.device_id.is_empty()
            && crate::controllers::normalize_device_id(&self.device_id).is_none()
        {
            // An unusable override is a hint, not a reason to drop the opt-in:
            // fall back to the automatic source at launch.
            self.device_id.clear();
        }
        Some(self)
    }
}

/// The stored opt-in of one game, normalised.
pub fn preference(
    stored: &BTreeMap<String, VirtualPadPreference>,
    game_id: i64,
) -> Option<VirtualPadPreference> {
    stored
        .get(&game_id.to_string())
        .cloned()
        .and_then(VirtualPadPreference::normalized)
}

/// A device the bridge can read: a connected joystick with an evdev node.
fn readable(device: &&InputDevice) -> bool {
    device.is_joystick && device.connected && device.event_path.is_some()
}

/// Pick the physical pad the bridge reads.
///
/// Order: the explicit override, then the game's preferred controller (#314),
/// then the first connected gamepad, then the first connected joystick. The
/// virtual pad replaces the pad the player already chose; it never silently
/// picks the wheel when a pad is present.
pub fn source_device(
    preference: &VirtualPadPreference,
    preferred: Option<&ControllerPreference>,
    devices: &[InputDevice],
) -> Option<InputDevice> {
    let by_id = |id: &str| {
        devices
            .iter()
            .find(|device| device.device_id.as_deref() == Some(id) && readable(device))
            .cloned()
    };
    if !preference.device_id.is_empty() {
        if let Some(device) = by_id(&preference.device_id) {
            return Some(device);
        }
    }
    if let Some(preferred) = preferred {
        if let Some(device) = by_id(&preferred.device_id) {
            return Some(device);
        }
    }
    devices
        .iter()
        .filter(readable)
        .find(|device| device.role == crate::controllers::DeviceRole::Gamepad)
        .cloned()
        .or_else(|| devices.iter().find(readable).cloned())
}

/// What the host and the device list allow, before any device is created.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualPadPlan {
    /// The game opted in.
    pub requested: bool,
    /// The bridge can run: host available and a readable source pad exists.
    pub enabled: bool,
    pub source: Option<InputDevice>,
    pub warning: Option<String>,
    pub summary: Option<String>,
}

/// Decide whether the bridge can run for this launch.
///
/// Pure: no `/dev/uinput`, no evdev. The caller passes the availability it just
/// probed, so the missing-permission path is testable.
pub fn plan(
    preference: Option<&VirtualPadPreference>,
    preferred: Option<&ControllerPreference>,
    devices: &[InputDevice],
    availability: &VirtualPadAvailability,
) -> VirtualPadPlan {
    let Some(preference) = preference.and_then(|entry| entry.clone().normalized()) else {
        return VirtualPadPlan::default();
    };
    // The source is resolved even when the host cannot create the device: the
    // UI shows which pad would be read, and the warning explains what is
    // missing. `enabled` is what gates the bridge.
    let source = source_device(&preference, preferred, devices);
    if availability.status != VirtualPadStatus::Available {
        return VirtualPadPlan {
            requested: true,
            source,
            warning: Some(format!(
                "The virtual gamepad is enabled for this game but unavailable: {}. The launch continues with the physical pads untouched.",
                availability.detail
            )),
            ..VirtualPadPlan::default()
        };
    }
    let Some(source) = source else {
        return VirtualPadPlan {
            requested: true,
            warning: Some(
                "The virtual gamepad is enabled for this game but no connected joystick has an evdev node to read; the launch continues with the physical pads untouched."
                    .to_string(),
            ),
            ..VirtualPadPlan::default()
        };
    };
    VirtualPadPlan {
        requested: true,
        enabled: true,
        summary: Some(format!(
            "virtual gamepad {VIRTUAL_PAD_NAME} reading {} ({})",
            source.label(),
            source.event_path.as_deref().unwrap_or_default()
        )),
        source: Some(source),
        warning: None,
    }
}

/// A launch's virtual-pad outcome: the environment to apply, the running
/// bridge to stop on exit, and the sentence for the log.
pub struct VirtualPadLaunch {
    pub controllers: ControllerLaunch,
    pub bridge: Option<VirtualPadBridge>,
    pub warning: Option<String>,
    pub summary: Option<String>,
}

impl VirtualPadLaunch {
    /// The option is off: the #314 environment is used unchanged.
    pub fn untouched(controllers: ControllerLaunch) -> Self {
        Self {
            controllers,
            bridge: None,
            warning: None,
            summary: None,
        }
    }
}

/// Fold the virtual-pad warning into the #314 launch so the log header written
/// by `launch_log_header` carries it: the header prints `ControllerLaunch` only.
fn warn_launch(mut launch: ControllerLaunch, warning: Option<String>) -> ControllerLaunch {
    launch.warning = match (launch.warning.take(), warning) {
        (None, None) => None,
        (Some(existing), None) => Some(existing),
        (None, Some(added)) => Some(added),
        (Some(existing), Some(added)) => Some(format!("{existing} {added}")),
    };
    launch
}

/// Probe the host, resolve the source pad and start the bridge.
///
/// `base` is what the #314 preference would have produced. It is used verbatim
/// when the bridge cannot run, so a failure never hides a physical pad from the
/// game.
pub fn prepare_launch(
    preference: Option<&VirtualPadPreference>,
    preferred: Option<&ControllerPreference>,
    devices: &[InputDevice],
    base: ControllerLaunch,
) -> VirtualPadLaunch {
    let Some(preference) = preference else {
        return VirtualPadLaunch::untouched(base);
    };
    let availability = availability();
    let plan = plan(Some(preference), preferred, devices, &availability);
    if !plan.requested {
        return VirtualPadLaunch::untouched(base);
    }
    let Some(source) = plan.source.filter(|_| plan.enabled) else {
        // The caller logs `warning` once, with the game id, and the merged copy
        // makes the launch log header carry the same sentence.
        let warning = plan.warning;
        return VirtualPadLaunch {
            controllers: warn_launch(base, warning.clone()),
            bridge: None,
            warning,
            summary: None,
        };
    };
    let event_path = match source.event_path.as_deref() {
        Some(path) => path,
        None => {
            let warning = Some(
                "The virtual gamepad is enabled for this game but the selected device has no evdev node; the launch continues with the physical pads untouched."
                    .to_string(),
            );
            return VirtualPadLaunch {
                controllers: warn_launch(base, warning.clone()),
                bridge: None,
                warning,
                summary: None,
            };
        }
    };
    let bridge = match start_for_device(Path::new(event_path)) {
        Ok(bridge) => bridge,
        Err(error) => {
            // The error is folded into the warning the caller logs.
            let warning = Some(format!(
                "The virtual gamepad could not be created ({error}); the launch continues with the physical pads untouched."
            ));
            return VirtualPadLaunch {
                controllers: warn_launch(base, warning.clone()),
                bridge: None,
                warning,
                summary: None,
            };
        }
    };
    // Hiding happens only once the device exists: a failed bridge must never
    // leave the game with no pad at all.
    let mut controllers = crate::controllers::launch_ignoring_all_pads(devices);
    if let Some(summary) = plan.summary.as_deref() {
        controllers.summary = Some(match controllers.summary.take() {
            Some(hidden) => format!("{summary}; {hidden}"),
            None => summary.to_string(),
        });
    }
    if controllers.warning.is_some() {
        tracing::warn!(
            warning = controllers.warning.as_deref().unwrap_or_default(),
            "some physical pads stay visible to the game"
        );
    }
    VirtualPadLaunch {
        controllers,
        bridge: Some(bridge),
        warning: None,
        summary: plan.summary,
    }
}

/// Open `/dev/uinput` and the source node, then run the bridge.
#[cfg(target_os = "linux")]
pub fn start_for_device(event_path: &Path) -> Result<VirtualPadBridge, String> {
    let backend = Box::new(uinput::UinputDevice::open(Path::new(UINPUT_PATH))?);
    let source = Box::new(evdev::EvdevDevice::open(event_path)?);
    VirtualPadBridge::start(backend, source, descriptor())
}

/// Non-Linux stub (#315): no backend exists, so the option reports unsupported
/// instead of pretending to work.
#[cfg(not(target_os = "linux"))]
pub fn start_for_device(_event_path: &Path) -> Result<VirtualPadBridge, String> {
    Err("The virtual gamepad is Linux-only for now.".to_string())
}

/* ------------------------------- uinput ---------------------------------- */

#[cfg(target_os = "linux")]
mod uinput {
    //! Minimal `uinput` writer. Struct layouts and ioctl numbers are from
    //! `include/uapi/linux/uinput.h` (kernel 4.5+, `UI_DEV_SETUP`).

    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{PadDescriptor, PadEvent, UinputBackend};

    // _IOW('U', 100, int), _IOW('U', 101, int), _IOW('U', 103, int)
    const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
    const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
    const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;
    // _IO('U', 1), _IO('U', 2)
    const UI_DEV_CREATE: libc::c_ulong = 0x5501;
    const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
    // _IOW('U', 3, struct uinput_setup): 92 bytes.
    const UI_DEV_SETUP: libc::c_ulong = 0x405C_5503;
    // _IOW('U', 4, struct uinput_abs_setup): 28 bytes.
    const UI_ABS_SETUP: libc::c_ulong = 0x401C_5504;

    // The kernel reads these through the ioctl before Rust ever would; the
    // fields are written, never read back, so the dead-code lint would fire.
    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputId {
        bustype: u16,
        vendor: u16,
        product: u16,
        version: u16,
    }

    #[allow(dead_code)]
    #[repr(C)]
    struct UinputSetup {
        id: InputId,
        name: [libc::c_char; 80],
        ff_effects_max: u32,
    }

    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct InputAbsInfo {
        value: i32,
        minimum: i32,
        maximum: i32,
        fuzz: i32,
        flat: i32,
        resolution: i32,
    }

    #[repr(C)]
    struct UinputAbsSetup {
        code: u16,
        absinfo: InputAbsInfo,
    }

    /// The open `/dev/uinput` fd and the created device.
    pub struct UinputDevice {
        fd: libc::c_int,
        created: bool,
    }

    impl UinputDevice {
        pub fn open(path: &Path) -> Result<Self, String> {
            let raw = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| format!("Invalid uinput path {}.", path.display()))?;
            // O_CLOEXEC: the game spawned next must never inherit this fd, or
            // the device would outlive the bridge that feeds it.
            let fd = unsafe {
                libc::open(
                    raw.as_ptr(),
                    libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(format!(
                    "Could not open {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self { fd, created: false })
        }
    }

    impl UinputBackend for UinputDevice {
        fn create(&mut self, descriptor: &PadDescriptor) -> Result<(), String> {
            let set = |request: libc::c_ulong, bit: u16| -> Result<(), String> {
                let code = libc::c_int::from(bit);
                let result = unsafe { libc::ioctl(self.fd, request, code) };
                if result < 0 {
                    return Err(format!(
                        "uinput capability {bit:#x} was refused: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                Ok(())
            };
            set(UI_SET_EVBIT, super::EV_KEY)?;
            set(UI_SET_EVBIT, super::EV_ABS)?;
            set(UI_SET_EVBIT, super::EV_SYN)?;
            for button in &descriptor.buttons {
                set(UI_SET_KEYBIT, *button)?;
            }
            for axis in &descriptor.axes {
                set(UI_SET_ABSBIT, axis.code)?;
                let setup = UinputAbsSetup {
                    code: axis.code,
                    absinfo: InputAbsInfo {
                        value: 0,
                        minimum: axis.min,
                        maximum: axis.max,
                        fuzz: axis.fuzz,
                        flat: axis.flat,
                        resolution: 0,
                    },
                };
                let result =
                    unsafe { libc::ioctl(self.fd, UI_ABS_SETUP, &setup as *const UinputAbsSetup) };
                if result < 0 {
                    return Err(format!(
                        "uinput axis {:#x} was refused: {}",
                        axis.code,
                        std::io::Error::last_os_error()
                    ));
                }
            }

            let mut name = [0 as libc::c_char; 80];
            for (slot, byte) in name.iter_mut().zip(descriptor.name.bytes()) {
                *slot = byte as libc::c_char;
            }
            let setup = UinputSetup {
                id: InputId {
                    bustype: descriptor.bustype,
                    vendor: descriptor.vendor,
                    product: descriptor.product,
                    version: descriptor.version,
                },
                name,
                ff_effects_max: 0,
            };
            let result =
                unsafe { libc::ioctl(self.fd, UI_DEV_SETUP, &setup as *const UinputSetup) };
            if result < 0 {
                return Err(format!(
                    "uinput device setup was refused: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let result = unsafe { libc::ioctl(self.fd, UI_DEV_CREATE) };
            if result < 0 {
                return Err(format!(
                    "uinput UI_DEV_CREATE failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            self.created = true;
            Ok(())
        }

        fn emit(&mut self, event: PadEvent) -> Result<(), String> {
            let event = match event {
                PadEvent::Key { code, value } => libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: super::EV_KEY,
                    code,
                    value,
                },
                PadEvent::Abs { code, value } => libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: super::EV_ABS,
                    code,
                    value,
                },
                PadEvent::Syn => libc::input_event {
                    time: libc::timeval {
                        tv_sec: 0,
                        tv_usec: 0,
                    },
                    type_: super::EV_SYN,
                    code: super::SYN_REPORT,
                    value: 0,
                },
            };
            let bytes = std::mem::size_of::<libc::input_event>();
            let written = unsafe {
                libc::write(
                    self.fd,
                    &event as *const libc::input_event as *const libc::c_void,
                    bytes,
                )
            };
            if written == bytes as libc::ssize_t {
                return Ok(());
            }
            let error = std::io::Error::last_os_error();
            // uinput has a small queue: a game that stops reading makes the
            // kernel return EAGAIN. Dropping one event is what every input
            // stack does; stopping the bridge would freeze the pad.
            if matches!(error.raw_os_error(), Some(libc::EAGAIN) | Some(libc::EINTR)) {
                return Ok(());
            }
            Err(format!("uinput write failed: {error}"))
        }

        fn destroy(&mut self) {
            if self.created {
                // Failure here only means the kernel already removed it (the
                // app is exiting); the fd close below is the real cleanup.
                unsafe {
                    libc::ioctl(self.fd, UI_DEV_DESTROY);
                }
                self.created = false;
            }
            if self.fd >= 0 {
                unsafe {
                    libc::close(self.fd);
                }
                self.fd = -1;
            }
        }
    }

    impl Drop for UinputDevice {
        fn drop(&mut self) {
            self.destroy();
        }
    }
}

/* -------------------------------- evdev ---------------------------------- */

#[cfg(target_os = "linux")]
mod evdev {
    //! Read-only evdev source. The device is **not** grabbed: the overlay keeps
    //! reading it, and a bridge crash cannot leave the pad dead.

    use std::collections::BTreeMap;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use super::{EvdevSource, SourceEvent, SourceRange, AXIS_CODES};

    /// `EVIOCGABS(axis)` = `_IOR('E', 0x40 + axis, struct input_absinfo)`
    /// (24-byte struct, read direction).
    fn eviocgabs(axis: u16) -> libc::c_ulong {
        0x8018_4500 + 0x40 + libc::c_ulong::from(axis)
    }

    pub struct EvdevDevice {
        fd: libc::c_int,
        path: std::path::PathBuf,
    }

    impl EvdevDevice {
        pub fn open(path: &Path) -> Result<Self, String> {
            let raw = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| format!("Invalid input device path {}.", path.display()))?;
            let fd = unsafe {
                libc::open(
                    raw.as_ptr(),
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(format!(
                    "Could not open {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self {
                fd,
                path: path.to_path_buf(),
            })
        }

        fn abs_info(&self, axis: u16) -> Option<libc::input_absinfo> {
            let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
            let result = unsafe { libc::ioctl(self.fd, eviocgabs(axis), &mut info) };
            (result == 0).then_some(info)
        }
    }

    impl EvdevSource for EvdevDevice {
        fn axis_ranges(&mut self) -> BTreeMap<u16, SourceRange> {
            let mut ranges = BTreeMap::new();
            for axis in AXIS_CODES {
                if let Some(info) = self.abs_info(axis) {
                    ranges.insert(
                        axis,
                        SourceRange {
                            min: info.minimum,
                            max: info.maximum,
                        },
                    );
                }
            }
            ranges
        }

        fn read(&mut self, timeout_ms: i32) -> Result<Vec<SourceEvent>, String> {
            let mut descriptor = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    return Ok(Vec::new());
                }
                return Err(format!("poll on {} failed: {error}", self.path.display()));
            }
            if ready == 0 {
                return Ok(Vec::new());
            }
            // A removed device makes poll return immediately, forever: report
            // it instead of spinning on a dead fd.
            let failed = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if descriptor.revents & failed != 0 {
                return Err(format!(
                    "{} was removed (the pad was unplugged)",
                    self.path.display()
                ));
            }
            let mut events = Vec::new();
            // Bounded: a burst is at most a frame of keys and axes.
            for _ in 0..64 {
                let mut event: libc::input_event = unsafe { std::mem::zeroed() };
                let bytes = std::mem::size_of::<libc::input_event>();
                let read = unsafe {
                    libc::read(
                        self.fd,
                        &mut event as *mut libc::input_event as *mut libc::c_void,
                        bytes,
                    )
                };
                if read == bytes as libc::ssize_t {
                    events.push(SourceEvent {
                        kind: event.type_,
                        code: event.code,
                        value: event.value,
                    });
                    continue;
                }
                if read < 0 {
                    let error = std::io::Error::last_os_error();
                    match error.raw_os_error() {
                        Some(libc::EAGAIN) => break,
                        Some(libc::ENODEV) => {
                            return Err(format!(
                                "{} was removed (the pad was unplugged)",
                                self.path.display()
                            ));
                        }
                        _ => {
                            return Err(format!("read on {} failed: {error}", self.path.display()));
                        }
                    }
                }
                // A short read on a non-blocking fd: nothing more right now.
                break;
            }
            Ok(events)
        }
    }

    impl Drop for EvdevDevice {
        fn drop(&mut self) {
            if self.fd >= 0 {
                unsafe {
                    libc::close(self.fd);
                }
                self.fd = -1;
            }
        }
    }
}

/* -------------------------- payload for the UI --------------------------- */

/// One game's virtual-pad state, returned with the controller diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VirtualPadView {
    pub status: VirtualPadStatus,
    pub detail: String,
    pub path: String,
    /// The stored per-game opt-in.
    pub enabled: bool,
    /// The stored override, empty for automatic.
    pub device_id: String,
    /// The pad the next launch would read, when one can be resolved.
    pub source: Option<String>,
    /// Why the opt-in cannot run on this host or with these devices.
    pub warning: Option<String>,
}

/// Build the UI payload from the stored preference and the current devices.
pub fn view(
    stored: &BTreeMap<String, VirtualPadPreference>,
    preferred: Option<&ControllerPreference>,
    game_id: i64,
    inventory: &ControllerInventory,
) -> VirtualPadView {
    let availability = availability();
    let stored_preference = preference(stored, game_id);
    let enabled = stored_preference.is_some();
    let device_id = stored_preference
        .as_ref()
        .map(|entry| entry.device_id.clone())
        .unwrap_or_default();
    let plan = plan(
        stored_preference.as_ref(),
        preferred,
        &inventory.devices,
        &availability,
    );
    VirtualPadView {
        status: availability.status,
        detail: availability.detail,
        path: availability.path,
        enabled,
        device_id,
        source: plan.source.as_ref().map(|device| device.label()),
        warning: plan.warning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    /* ------------------------------ helpers ------------------------------ */

    fn range(min: i32, max: i32) -> SourceRange {
        SourceRange { min, max }
    }

    fn ranges() -> BTreeMap<u16, SourceRange> {
        let mut map = BTreeMap::new();
        map.insert(ABS_X, range(-32768, 32767));
        map.insert(ABS_Y, range(-32768, 32767));
        map.insert(ABS_RX, range(0, 255));
        map.insert(ABS_RY, range(0, 255));
        map.insert(ABS_Z, range(0, 255));
        map.insert(ABS_RZ, range(0, 255));
        map.insert(ABS_HAT0X, range(-1, 1));
        map.insert(ABS_HAT0Y, range(-1, 1));
        map
    }

    fn pad_device(
        name: &str,
        id: Option<&str>,
        event: u32,
        js: u32,
        role: crate::controllers::DeviceRole,
    ) -> InputDevice {
        let (vendor, product) = id
            .and_then(|value| value.split_once(':'))
            .map(|(vendor, product)| (Some(vendor.to_string()), Some(product.to_string())))
            .unwrap_or((None, None));
        InputDevice {
            name: name.to_string(),
            vendor,
            product,
            device_id: id.map(str::to_string),
            js_index: Some(js),
            event_index: Some(event),
            js_path: Some(format!("/dev/input/js{js}")),
            event_path: Some(format!("/dev/input/event{event}")),
            connected: true,
            role,
            is_joystick: true,
        }
    }

    fn wheel() -> InputDevice {
        pad_device(
            "Logitech G29 Driving Force Racing Wheel",
            Some("046d:c260"),
            8,
            0,
            crate::controllers::DeviceRole::Wheel,
        )
    }

    fn pad() -> InputDevice {
        pad_device(
            "Microsoft X-Box 360 pad",
            Some("045e:028e"),
            261,
            2,
            crate::controllers::DeviceRole::Gamepad,
        )
    }

    fn available() -> VirtualPadAvailability {
        VirtualPadAvailability {
            status: VirtualPadStatus::Available,
            path: UINPUT_PATH.to_string(),
            detail: "writable".to_string(),
        }
    }

    fn enabled_preference() -> VirtualPadPreference {
        VirtualPadPreference {
            enabled: true,
            device_id: String::new(),
        }
    }

    /* ---------------------------- descriptor ----------------------------- */

    #[test]
    fn the_descriptor_advertises_an_xbox_style_pad() {
        let descriptor = descriptor();
        assert_eq!(descriptor.name, "Ludex Virtual Gamepad");
        assert_eq!(descriptor.vendor, 0x4c44);
        assert_eq!(descriptor.product, 0x0001);
        assert_eq!(descriptor.bustype, 0x0003);
        assert_eq!(
            descriptor.buttons,
            vec![
                BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START,
                BTN_MODE, BTN_THUMBL, BTN_THUMBR
            ]
        );
        let axis = |code: u16| {
            descriptor
                .axes
                .iter()
                .find(|axis| axis.code == code)
                .copied()
        };
        assert_eq!(
            axis(ABS_X).map(|axis| (axis.min, axis.max)),
            Some((-32768, 32767))
        );
        assert_eq!(
            axis(ABS_RY).map(|axis| (axis.min, axis.max)),
            Some((-32768, 32767))
        );
        assert_eq!(axis(ABS_Z).map(|axis| (axis.min, axis.max)), Some((0, 255)));
        assert_eq!(
            axis(ABS_RZ).map(|axis| (axis.min, axis.max)),
            Some((0, 255))
        );
        assert_eq!(
            axis(ABS_HAT0X).map(|axis| (axis.min, axis.max)),
            Some((-1, 1))
        );
        assert_eq!(
            axis(ABS_HAT0Y).map(|axis| (axis.min, axis.max)),
            Some((-1, 1))
        );
        assert_eq!(descriptor.axes.len(), AXIS_CODES.len());
    }

    /* --------------------------- translation ----------------------------- */

    #[test]
    fn buttons_pass_through_unchanged() {
        let descriptor = descriptor();
        let press = translate(
            SourceEvent {
                kind: EV_KEY,
                code: BTN_SOUTH,
                value: 1,
            },
            &ranges(),
            &descriptor,
        );
        assert_eq!(
            press,
            vec![PadEvent::Key {
                code: BTN_SOUTH,
                value: 1
            }]
        );
        let release = translate(
            SourceEvent {
                kind: EV_KEY,
                code: BTN_START,
                value: 0,
            },
            &ranges(),
            &descriptor,
        );
        assert_eq!(
            release,
            vec![PadEvent::Key {
                code: BTN_START,
                value: 0
            }]
        );
        // A key the virtual pad does not advertise is dropped, not invented.
        assert!(translate(
            SourceEvent {
                kind: EV_KEY,
                code: 0x2ff,
                value: 1,
            },
            &ranges(),
            &descriptor,
        )
        .is_empty());
    }

    #[test]
    fn axes_pass_through_without_a_deadzone() {
        let descriptor = descriptor();
        for value in [0, 1, 3, 127, 128, -1, -32768, 32767] {
            let events = translate(
                SourceEvent {
                    kind: EV_ABS,
                    code: ABS_X,
                    value,
                },
                &ranges(),
                &descriptor,
            );
            assert_eq!(
                events,
                vec![PadEvent::Abs { code: ABS_X, value }],
                "an equal source range must be copied exactly, value {value}"
            );
        }
    }

    #[test]
    fn source_ranges_are_rescaled_to_the_descriptor() {
        let descriptor = descriptor();
        // A pad that reports its sticks as 0..255 (many cheap pads do) still
        // produces a full-range Xbox axis.
        assert_eq!(
            translate(
                SourceEvent {
                    kind: EV_ABS,
                    code: ABS_RX,
                    value: 0,
                },
                &ranges(),
                &descriptor,
            ),
            vec![PadEvent::Abs {
                code: ABS_RX,
                value: -32768
            }]
        );
        assert_eq!(
            translate(
                SourceEvent {
                    kind: EV_ABS,
                    code: ABS_RX,
                    value: 255,
                },
                &ranges(),
                &descriptor,
            ),
            vec![PadEvent::Abs {
                code: ABS_RX,
                value: 32767
            }]
        );
        let centre = translate(
            SourceEvent {
                kind: EV_ABS,
                code: ABS_RX,
                value: 128,
            },
            &ranges(),
            &descriptor,
        );
        assert_eq!(
            centre,
            vec![PadEvent::Abs {
                code: ABS_RX,
                value: 128
            }]
        );
    }

    #[test]
    fn an_unknown_source_range_is_clamped_not_guessed() {
        let descriptor = descriptor();
        // No EVIOCGABS answer (range missing): values are clamped to the
        // virtual range, never rescaled through a made-up span.
        assert_eq!(
            translate(
                SourceEvent {
                    kind: EV_ABS,
                    code: ABS_Z,
                    value: 5000,
                },
                &BTreeMap::new(),
                &descriptor,
            ),
            vec![PadEvent::Abs {
                code: ABS_Z,
                value: 255
            }]
        );
        assert_eq!(
            translate(
                SourceEvent {
                    kind: EV_ABS,
                    code: ABS_HAT0Y,
                    value: 9,
                },
                &BTreeMap::new(),
                &descriptor,
            ),
            vec![PadEvent::Abs {
                code: ABS_HAT0Y,
                value: 1
            }]
        );
    }

    #[test]
    fn d_pad_buttons_become_a_hat() {
        let descriptor = descriptor();
        let case = |code: u16, value: i32| {
            translate(
                SourceEvent {
                    kind: EV_KEY,
                    code,
                    value,
                },
                &ranges(),
                &descriptor,
            )
        };
        assert_eq!(
            case(BTN_DPAD_UP, 1),
            vec![PadEvent::Abs {
                code: ABS_HAT0Y,
                value: -1
            }]
        );
        assert_eq!(
            case(BTN_DPAD_DOWN, 1),
            vec![PadEvent::Abs {
                code: ABS_HAT0Y,
                value: 1
            }]
        );
        assert_eq!(
            case(BTN_DPAD_LEFT, 1),
            vec![PadEvent::Abs {
                code: ABS_HAT0X,
                value: -1
            }]
        );
        assert_eq!(
            case(BTN_DPAD_RIGHT, 1),
            vec![PadEvent::Abs {
                code: ABS_HAT0X,
                value: 1
            }]
        );
        assert_eq!(
            case(BTN_DPAD_UP, 0),
            vec![PadEvent::Abs {
                code: ABS_HAT0Y,
                value: 0
            }]
        );
        // A hat event from a pad that already reports a hat is kept as-is.
        assert_eq!(
            translate(
                SourceEvent {
                    kind: EV_ABS,
                    code: ABS_HAT0X,
                    value: -1,
                },
                &ranges(),
                &descriptor,
            ),
            vec![PadEvent::Abs {
                code: ABS_HAT0X,
                value: -1
            }]
        );
    }

    #[test]
    fn only_syn_report_and_known_axes_are_forwarded() {
        let descriptor = descriptor();
        assert_eq!(
            translate(
                SourceEvent {
                    kind: EV_SYN,
                    code: SYN_REPORT,
                    value: 0,
                },
                &ranges(),
                &descriptor,
            ),
            vec![PadEvent::Syn]
        );
        // MSC_SCAN, EV_FF, EV_REL: nothing.
        for kind in [0x04u16, 0x15, 0x02] {
            assert!(translate(
                SourceEvent {
                    kind,
                    code: 0,
                    value: 0,
                },
                &ranges(),
                &descriptor,
            )
            .is_empty());
        }
    }

    /* ----------------------------- lifecycle ----------------------------- */

    #[derive(Default)]
    struct Recording {
        created: AtomicUsize,
        destroyed: AtomicUsize,
        events: Mutex<Vec<PadEvent>>,
    }

    struct FakeBackend {
        record: Arc<Recording>,
    }

    impl UinputBackend for FakeBackend {
        fn create(&mut self, descriptor: &PadDescriptor) -> Result<(), String> {
            assert_eq!(descriptor.name, VIRTUAL_PAD_NAME);
            self.record.created.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn emit(&mut self, event: PadEvent) -> Result<(), String> {
            self.record
                .events
                .lock()
                .map_err(|_| "recorder poisoned".to_string())?
                .push(event);
            Ok(())
        }

        fn destroy(&mut self) {
            self.record.destroyed.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct FakeSource {
        batches: Vec<Vec<SourceEvent>>,
        sent: usize,
    }

    impl EvdevSource for FakeSource {
        fn axis_ranges(&mut self) -> BTreeMap<u16, SourceRange> {
            ranges()
        }

        fn read(&mut self, _timeout_ms: i32) -> Result<Vec<SourceEvent>, String> {
            if self.sent < self.batches.len() {
                let batch = self.batches[self.sent].clone();
                self.sent += 1;
                std::thread::sleep(Duration::from_millis(2));
                return Ok(batch);
            }
            std::thread::sleep(Duration::from_millis(5));
            Ok(Vec::new())
        }
    }

    fn wait_for(record: &Recording, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if record.events.lock().map(|events| events.len()).unwrap_or(0) >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the bridge did not translate the events in time");
    }

    #[test]
    fn the_bridge_copies_events_and_stops_idempotently() {
        let record = Arc::new(Recording::default());
        let backend = Box::new(FakeBackend {
            record: Arc::clone(&record),
        });
        let source = Box::new(FakeSource {
            batches: vec![
                vec![SourceEvent {
                    kind: EV_KEY,
                    code: BTN_EAST,
                    value: 1,
                }],
                vec![SourceEvent {
                    kind: EV_SYN,
                    code: SYN_REPORT,
                    value: 0,
                }],
            ],
            sent: 0,
        });
        let bridge = VirtualPadBridge::start(backend, source, descriptor()).unwrap();
        assert!(bridge.is_running());
        wait_for(&record, 2);
        bridge.stop();
        assert!(!bridge.is_running());
        // Stop is idempotent: the session teardown calls it again.
        bridge.stop();
        assert_eq!(record.created.load(Ordering::SeqCst), 1);
        assert_eq!(record.destroyed.load(Ordering::SeqCst), 1);
        let events = record.events.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                PadEvent::Key {
                    code: BTN_EAST,
                    value: 1
                },
                PadEvent::Syn
            ]
        );
    }

    #[test]
    fn a_failing_create_never_starts_a_thread() {
        struct Broken;
        impl UinputBackend for Broken {
            fn create(&mut self, _descriptor: &PadDescriptor) -> Result<(), String> {
                Err("permission denied".to_string())
            }
            fn emit(&mut self, _event: PadEvent) -> Result<(), String> {
                unreachable!("no event after a failed create")
            }
            fn destroy(&mut self) {}
        }
        struct Empty;
        impl EvdevSource for Empty {
            fn axis_ranges(&mut self) -> BTreeMap<u16, SourceRange> {
                BTreeMap::new()
            }
            fn read(&mut self, _timeout_ms: i32) -> Result<Vec<SourceEvent>, String> {
                Ok(Vec::new())
            }
        }
        let error = match VirtualPadBridge::start(Box::new(Broken), Box::new(Empty), descriptor()) {
            Ok(_) => panic!("a broken backend must fail the start"),
            Err(error) => error,
        };
        assert!(error.contains("permission denied"));
    }

    /* ------------------------------ gating ------------------------------- */

    #[test]
    fn per_game_opt_in_gates_the_bridge() {
        let devices = vec![wheel(), pad()];
        let off = plan(None, None, &devices, &available());
        assert!(!off.requested);
        assert!(!off.enabled);

        let disabled = VirtualPadPreference {
            enabled: false,
            device_id: String::new(),
        };
        assert!(!plan(Some(&disabled), None, &devices, &available()).requested);

        let on = plan(Some(&enabled_preference()), None, &devices, &available());
        assert!(on.requested);
        assert!(on.enabled);
        // The wheel is first in the list; the gamepad is still chosen.
        assert_eq!(
            on.source
                .as_ref()
                .and_then(|device| device.device_id.as_deref()),
            Some("045e:028e")
        );
    }

    #[test]
    fn the_preferred_controller_is_the_source_pad() {
        let devices = vec![wheel(), pad()];
        let other = pad_device(
            "Sony DualSense",
            Some("054c:0ce6"),
            262,
            3,
            crate::controllers::DeviceRole::Gamepad,
        );
        let mut devices = devices;
        devices.push(other);
        let preferred = ControllerPreference {
            device_id: "054c:0ce6".into(),
            name: "Sony DualSense".into(),
        };
        let plan = plan(
            Some(&enabled_preference()),
            Some(&preferred),
            &devices,
            &available(),
        );
        assert_eq!(
            plan.source
                .as_ref()
                .and_then(|device| device.device_id.as_deref()),
            Some("054c:0ce6")
        );
    }

    #[test]
    fn an_explicit_override_wins_over_the_preference() {
        let devices = vec![wheel(), pad()];
        let preference = VirtualPadPreference {
            enabled: true,
            device_id: "046D:C260".into(),
        };
        let preferred = ControllerPreference {
            device_id: "045e:028e".into(),
            name: String::new(),
        };
        let plan = plan(Some(&preference), Some(&preferred), &devices, &available());
        assert_eq!(
            plan.source
                .as_ref()
                .and_then(|device| device.device_id.as_deref()),
            Some("046d:c260")
        );
    }

    #[test]
    fn missing_uinput_falls_back_with_a_warning() {
        let devices = vec![wheel(), pad()];
        let unavailable = VirtualPadAvailability {
            status: VirtualPadStatus::MissingPermission,
            path: UINPUT_PATH.to_string(),
            detail: format!("{UINPUT_PATH} exists but this account cannot write to it."),
        };
        let plan = plan(Some(&enabled_preference()), None, &devices, &unavailable);
        assert!(plan.requested);
        assert!(!plan.enabled);
        // The pad that *would* be read is still reported, so the UI can explain
        // the setup instead of showing an empty selection.
        assert_eq!(
            plan.source
                .as_ref()
                .and_then(|device| device.device_id.as_deref()),
            Some("045e:028e")
        );
        let warning = plan.warning.expect("a warning is reported");
        assert!(warning.contains("/dev/uinput"));
        assert!(warning.contains("physical pads untouched"));

        // The launch keeps the #314 environment: nothing is hidden when the
        // bridge cannot run, and the warning reaches the launch log header.
        let base = crate::controllers::launch_for_preference(None, &devices);
        let launch = prepare_launch(Some(&enabled_preference()), None, &devices, base.clone());
        assert!(launch.bridge.is_none());
        assert_eq!(launch.controllers.env, base.env);
        assert!(launch.warning.is_some());
        assert!(launch
            .controllers
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("/dev/uinput"));
    }

    #[test]
    fn no_connected_pad_falls_back_with_a_warning() {
        let devices = [pad_device(
            "Microsoft X-Box 360 pad",
            Some("045e:028e"),
            261,
            2,
            crate::controllers::DeviceRole::Gamepad,
        )];
        let mut disconnected = devices[0].clone();
        disconnected.connected = false;
        let plan = plan(
            Some(&enabled_preference()),
            None,
            &[disconnected],
            &available(),
        );
        assert!(plan.requested);
        assert!(!plan.enabled);
        assert!(plan
            .warning
            .unwrap_or_default()
            .contains("no connected joystick"));
    }

    #[test]
    fn an_unsupported_platform_is_reported_not_attempted() {
        let unsupported = VirtualPadAvailability {
            status: VirtualPadStatus::UnsupportedPlatform,
            path: String::new(),
            detail: "Linux-only".to_string(),
        };
        let plan = plan(Some(&enabled_preference()), None, &[pad()], &unsupported);
        assert!(plan.requested);
        assert!(!plan.enabled);
        assert!(plan.warning.unwrap_or_default().contains("Linux"));
    }

    #[test]
    fn a_disabled_or_invalid_entry_normalises_away() {
        assert!(VirtualPadPreference::default().normalized().is_none());
        let invalid = VirtualPadPreference {
            enabled: true,
            device_id: "0000:0000".into(),
        };
        let stored = invalid.clone().normalized().expect("the opt-in survives");
        assert!(
            stored.device_id.is_empty(),
            "the unusable override is dropped"
        );
        let mut map = BTreeMap::new();
        map.insert("7".to_string(), invalid);
        assert_eq!(preference(&map, 7).map(|entry| entry.enabled), Some(true));
        assert!(preference(&map, 8).is_none());
    }

    #[test]
    fn the_view_reports_the_stored_state_and_the_source() {
        let inventory = ControllerInventory {
            available: true,
            source: "/proc/bus/input/devices".to_string(),
            note: None,
            devices: vec![wheel(), pad()],
        };
        let mut stored = BTreeMap::new();
        let before = view(&stored, None, 5, &inventory);
        assert!(!before.enabled);
        assert_eq!(before.status, availability().status);
        assert!(before.source.is_none());

        stored.insert(
            "5".to_string(),
            VirtualPadPreference {
                enabled: true,
                device_id: String::new(),
            },
        );
        let after = view(&stored, None, 5, &inventory);
        assert!(after.enabled);
        assert_eq!(
            after.source.as_deref(),
            Some("Microsoft X-Box 360 pad (045e:028e, js2)")
        );
    }

    /* ------------------------- non-Linux compile gate --------------------- */

    /// On a non-Linux build the whole backend is a stub: no ioctl, no device.
    /// The test is compiled only there, so the Linux run of `cargo test` does
    /// not pretend to cover it.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn the_non_linux_stub_reports_unsupported() {
        assert_eq!(availability().status, VirtualPadStatus::UnsupportedPlatform);
        assert!(start_for_device(Path::new("/dev/input/event0")).is_err());
    }
}
