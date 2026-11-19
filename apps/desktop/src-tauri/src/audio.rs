//! Audio output routing per game (#289).
//!
//! Games open their audio stream on the default sink, which is wrong on a desk
//! with a headset, a TV over HDMI and a capture card. This module lists the
//! PipeWire/Pulse sinks, stores a per-game sink, and moves the game's own
//! sink-inputs to it once they exist.
//!
//! Everything that talks to the host goes through [`AudioRunner`], so the
//! `pactl` command lines, the retry loop and the fallbacks are unit-tested
//! without PipeWire. The parsing is pure: `pactl list short sinks`,
//! `pactl list sinks`, `pactl list sink-inputs` and `get-default-sink` all have
//! text outputs that differ slightly between PulseAudio and PipeWire, so the
//! parsers are defensive and a malformed block is skipped, never fatal.
//!
//! Routing never invents a device: a stored sink that is no longer listed is
//! replaced by the profile default (the global override, then the OS default)
//! and the fallback is reported once. Moving a stream is best effort, and a
//! game whose sink disappears keeps playing on whatever the sound server
//! picks, instead of being muted forever.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::process_tree::{subtree_pids, ProcessInfo};
use crate::state::AppState;

/// One volume step for the console OSD and the Settings rows.
pub const VOLUME_STEP_PERCENT: u8 = 5;
/// The launch environment key the older tier-2 audio switch writes (#299).
/// The router treats it as a per-game sink when `audioSink` is empty.
pub const PULSE_SINK_ENV: &str = "PULSE_SINK";
/// How many times the router looks for a game's audio stream.
pub const ROUTE_MAX_ATTEMPTS: usize = 24;
/// Delay between two route attempts; a bounded window so a game that never
/// opens a stream costs a few seconds, not a thread for the whole session.
pub const ROUTE_RETRY_DELAY: Duration = Duration::from_millis(250);

/* ------------------------------- data model ------------------------------ */

/// One output device reported by `pactl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkInfo {
    /// PulseAudio/PipeWire index, the stable handle `move-sink-input` uses.
    pub index: u32,
    /// Full sink name (`alsa_output.pci-0000_00_1f.3.analog-stereo`).
    pub name: String,
    /// Human description (`Built-in Audio Analog Stereo`); the name when the
    /// server did not report one.
    pub description: String,
    /// True for `pactl get-default-sink`.
    pub is_default: bool,
}

impl SinkInfo {
    /// The label the UI shows: the description, or the technical name when it
    /// is the only thing available.
    pub fn label(&self) -> String {
        if self.description.trim().is_empty() {
            self.name.clone()
        } else {
            self.description.clone()
        }
    }
}

/// A probe result: the sinks, the OS default and which tools exist.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SinkList {
    pub sinks: Vec<SinkInfo>,
    pub default_sink: String,
    /// True when `pactl` exists on this host.
    pub pactl: bool,
    /// `pw-play` or `paplay`, whichever is installed.
    pub player: Option<String>,
}

/// The shape the web UI reads from `audio_sinks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioOverview {
    pub sinks: Vec<SinkOption>,
    /// The sound server's default sink name; empty when there is none.
    pub default_sink: String,
    /// Ludex's global override; empty means "follow the system default".
    pub global_default: String,
    /// False when `pactl` is missing: audio routing is unavailable.
    pub pactl: bool,
    /// The installed tone player, if any.
    pub player: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SinkOption {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub is_default: bool,
}

impl From<SinkInfo> for SinkOption {
    fn from(sink: SinkInfo) -> Self {
        Self {
            index: sink.index,
            name: sink.name,
            description: sink.description,
            is_default: sink.is_default,
        }
    }
}

impl AudioOverview {
    fn from_list(list: SinkList, global_default: &str) -> Self {
        Self {
            sinks: list.sinks.into_iter().map(SinkOption::from).collect(),
            default_sink: list.default_sink,
            global_default: global_default.trim().to_string(),
            pactl: list.pactl,
            player: list.player,
        }
    }
}

/// One libpulse stream, as parsed from `pactl list sink-inputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkInput {
    /// Sink-input index (`Sink Input #41`).
    pub index: u32,
    /// Index of the sink it currently plays on.
    pub sink_index: Option<u32>,
    /// `application.process.id`, the bridge to the process tree.
    pub pid: Option<u32>,
}

/// Why the router could not use the exact sink it was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    /// The per-game sink is gone; the profile default took over.
    GameSinkMissing,
    /// The global override is gone; the OS default took over.
    DefaultSinkMissing,
    /// No usable sink at all (no sound server, no devices).
    NoSink,
}

/// The fallback a launch reports to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteNotice {
    pub kind: NoticeKind,
    pub requested: String,
    pub used: Option<String>,
}

/// The sink a launch should route to, plus the fallback to announce.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutePlan {
    pub sink: Option<SinkInfo>,
    pub notice: Option<RouteNotice>,
}

/// Event emitted on `audio://notice`; the toast/console notice translates it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AudioNoticeEvent {
    game_id: i64,
    kind: NoticeKind,
    requested: String,
    used: Option<String>,
}

/// Live volume and mute state of one sink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeState {
    /// Sink the state belongs to; empty when nothing is available.
    pub sink: String,
    /// Human label shown by the OSD.
    pub label: String,
    /// 0..=100, or 0 when unknown.
    pub percent: u8,
    pub muted: bool,
    /// False when no device, no `pactl` or a failed read.
    pub available: bool,
    pub pactl: bool,
}

/// What the console OSD asks the sound server to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeAction {
    Up,
    Down,
    ToggleMute,
}

impl VolumeAction {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "up" => Some(VolumeAction::Up),
            "down" => Some(VolumeAction::Down),
            "toggleMute" | "mute" => Some(VolumeAction::ToggleMute),
            _ => None,
        }
    }

    /// The exact `pactl` arguments for this action on `sink`.
    pub fn args(self, sink: &str) -> Vec<String> {
        match self {
            VolumeAction::Up => vec![
                "set-sink-volume".to_string(),
                sink.to_string(),
                format!("+{VOLUME_STEP_PERCENT}%"),
            ],
            VolumeAction::Down => vec![
                "set-sink-volume".to_string(),
                sink.to_string(),
                format!("-{VOLUME_STEP_PERCENT}%"),
            ],
            VolumeAction::ToggleMute => {
                vec![
                    "set-sink-mute".to_string(),
                    sink.to_string(),
                    "toggle".to_string(),
                ]
            }
        }
    }
}

/* -------------------------------- parsing -------------------------------- */

/// Parse `pactl list short sinks` into `(index, name)` pairs.
pub fn parse_short_sinks(output: &str) -> Vec<(u32, String)> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let index = fields.next()?.parse::<u32>().ok()?;
            let name = fields.next()?.trim();
            (!name.is_empty()).then(|| (index, name.to_string()))
        })
        .collect()
}

/// Parse `pactl list sinks` into a `name -> Description` map.
///
/// Only the `Name:` and `Description:` lines matter; every other property is
/// ignored so a new PipeWire field cannot break the parser.
pub fn parse_sink_descriptions(output: &str) -> BTreeMap<String, String> {
    let mut descriptions = BTreeMap::new();
    let mut name: Option<String> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Sink #") {
            name = None;
        } else if let Some(value) = trimmed.strip_prefix("Name:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = trimmed.strip_prefix("Description:") {
            if let Some(current) = name.as_deref() {
                let description = value.trim();
                if !description.is_empty() {
                    descriptions.insert(current.to_string(), description.to_string());
                }
            }
        }
    }
    descriptions
}

/// Combine the short list and the descriptions into sink records.
pub fn compose_sinks(
    short: &[(u32, String)],
    descriptions: &BTreeMap<String, String>,
    default_sink: &str,
) -> Vec<SinkInfo> {
    let default_sink = default_sink.trim();
    short
        .iter()
        .map(|(index, name)| SinkInfo {
            index: *index,
            name: name.clone(),
            description: descriptions
                .get(name)
                .cloned()
                .unwrap_or_else(|| name.clone()),
            is_default: !default_sink.is_empty() && name == default_sink,
        })
        .collect()
}

/// Parse the first non-empty line of `pactl get-default-sink`.
pub fn parse_default_sink(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Parse `pactl list sink-inputs` into one record per stream.
///
/// `application.process.id` is quoted on both PulseAudio and PipeWire. A block
/// without it keeps `pid: None` and is never matched to a game.
pub fn parse_sink_inputs(output: &str) -> Vec<SinkInput> {
    let mut inputs = Vec::new();
    let mut current: Option<SinkInput> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Sink Input #") {
            if let Some(input) = current.take() {
                inputs.push(input);
            }
            current = rest.trim().parse::<u32>().ok().map(|index| SinkInput {
                index,
                sink_index: None,
                pid: None,
            });
            continue;
        }
        let Some(input) = current.as_mut() else {
            continue;
        };
        if let Some(value) = trimmed.strip_prefix("Sink:") {
            input.sink_index = value.trim().parse::<u32>().ok();
        } else if trimmed.starts_with("application.process.id") {
            if let Some((_, value)) = trimmed.split_once('=') {
                input.pid = value.trim().trim_matches('"').parse::<u32>().ok();
            }
        }
    }
    if let Some(input) = current.take() {
        inputs.push(input);
    }
    inputs
}

/// The sink-inputs whose owning PID is in the game's process tree.
pub fn matching_sink_inputs(inputs: &[SinkInput], pids: &BTreeSet<u32>) -> Vec<SinkInput> {
    inputs
        .iter()
        .copied()
        .filter(|input| input.pid.is_some_and(|pid| pids.contains(&pid)))
        .collect()
}

/* ------------------------------- selection ------------------------------- */

/// Pick the sink for a game and the fallback to report.
///
/// Order: the per-game sink, then the global override, then the sound server's
/// default. A missing per-game sink falls back to the profile default and is
/// reported; a missing global override falls back to the OS default.
pub fn resolve_plan(game_sink: &str, global_default: &str, list: &SinkList) -> RoutePlan {
    let find = |name: &str| list.sinks.iter().find(|sink| sink.name == name).cloned();
    let os_default = || {
        find(&list.default_sink)
            .or_else(|| list.sinks.iter().find(|sink| sink.is_default).cloned())
            .or_else(|| list.sinks.first().cloned())
    };

    let game_sink = game_sink.trim();
    let global_default = global_default.trim();

    if !game_sink.is_empty() {
        if let Some(sink) = find(game_sink) {
            return RoutePlan {
                sink: Some(sink),
                notice: None,
            };
        }
        let profile_default = if global_default.is_empty() {
            os_default()
        } else {
            find(global_default).or_else(os_default)
        };
        return match profile_default {
            Some(sink) => RoutePlan {
                notice: Some(RouteNotice {
                    kind: NoticeKind::GameSinkMissing,
                    requested: game_sink.to_string(),
                    used: Some(sink.label()),
                }),
                sink: Some(sink),
            },
            None => RoutePlan {
                sink: None,
                notice: Some(RouteNotice {
                    kind: NoticeKind::NoSink,
                    requested: game_sink.to_string(),
                    used: None,
                }),
            },
        };
    }

    if !global_default.is_empty() {
        if let Some(sink) = find(global_default) {
            return RoutePlan {
                sink: Some(sink),
                notice: None,
            };
        }
        return match os_default() {
            Some(sink) => RoutePlan {
                notice: Some(RouteNotice {
                    kind: NoticeKind::DefaultSinkMissing,
                    requested: global_default.to_string(),
                    used: Some(sink.label()),
                }),
                sink: Some(sink),
            },
            None => RoutePlan {
                sink: None,
                notice: Some(RouteNotice {
                    kind: NoticeKind::NoSink,
                    requested: global_default.to_string(),
                    used: None,
                }),
            },
        };
    }

    match os_default() {
        Some(sink) => RoutePlan {
            sink: Some(sink),
            notice: None,
        },
        None => RoutePlan {
            sink: None,
            notice: Some(RouteNotice {
                kind: NoticeKind::NoSink,
                requested: String::new(),
                used: None,
            }),
        },
    }
}

/* ------------------------------ retry loop ------------------------------- */

/// What one bounded routing pass did.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RouteReport {
    /// Sink-input indices the pass asked the server to move, in order.
    pub moved: Vec<u32>,
    pub attempts: usize,
    /// True when every matching stream was already on the target sink.
    pub done: bool,
}

/// Move a game's streams, retrying while the stream appears.
///
/// `observe` returns the current sink-inputs plus the game's PID set, and
/// `move_input` performs one `move-sink-input`. Production re-probes between
/// attempts; tests feed a scripted sequence. A stream that appears on the
/// second attempt is therefore moved without any PipeWire in the test.
#[cfg(test)]
pub fn run_route_loop(
    max_attempts: usize,
    target_index: u32,
    mut observe: impl FnMut() -> Result<(Vec<SinkInput>, BTreeSet<u32>), String>,
    mut move_input: impl FnMut(u32) -> Result<(), String>,
) -> Result<RouteReport, String> {
    let mut report = RouteReport::default();
    for attempt in 0..max_attempts.max(1) {
        report.attempts = attempt + 1;
        let (inputs, pids) = observe()?;
        let matching = matching_sink_inputs(&inputs, &pids);
        if matching.is_empty() {
            continue;
        }
        let pending: Vec<u32> = matching
            .iter()
            .filter(|input| input.sink_index != Some(target_index))
            .map(|input| input.index)
            .collect();
        if pending.is_empty() {
            report.done = true;
            return Ok(report);
        }
        for index in pending {
            move_input(index)?;
            if !report.moved.contains(&index) {
                report.moved.push(index);
            }
        }
    }
    Ok(report)
}

/* -------------------------------- runner --------------------------------- */

/// Every host call the audio module makes, injectable for tests.
pub trait AudioRunner: Send + Sync {
    /// Run a command and capture stdout; a failed exit is an error.
    fn run(&self, program: &str, args: &[String]) -> Result<String, String>;
    /// Start a command without waiting (the test tone).
    fn run_detached(&self, program: &str, args: &[String]) -> Result<(), String>;
    /// True when the program exists on this host.
    fn available(&self, program: &str) -> bool;
}

/// The real host runner: `Command` with no shell, so a sink name can never be
/// interpreted as shell syntax.
pub struct SystemRunner;

impl AudioRunner for SystemRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("Could not run {program}: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("{program} exited with {}", output.status)
            } else {
                format!("{program}: {stderr}")
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn run_detached(&self, program: &str, args: &[String]) -> Result<(), String> {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not start {program}: {error}"))
    }

    fn available(&self, program: &str) -> bool {
        if program.contains('/') || program.contains('\\') {
            return Path::new(program).is_file();
        }
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| {
            let candidate = dir.join(program);
            candidate.is_file() || (cfg!(windows) && dir.join(format!("{program}.exe")).is_file())
        })
    }
}

/* -------------------------------- service -------------------------------- */

/// The audio surface used by the commands and the launch router.
pub struct AudioService {
    runner: Arc<dyn AudioRunner>,
}

impl AudioService {
    pub fn system() -> Self {
        Self {
            runner: Arc::new(SystemRunner),
        }
    }

    #[cfg(test)]
    fn with_runner(runner: Arc<dyn AudioRunner>) -> Self {
        Self { runner }
    }

    /// Detect the player once; `pw-play` is preferred on a PipeWire host.
    fn player(&self) -> Option<&'static str> {
        if self.runner.available("pw-play") {
            Some("pw-play")
        } else if self.runner.available("paplay") {
            Some("paplay")
        } else {
            None
        }
    }

    /// Probe the sinks. A missing `pactl` is data (`pactl: false`), not an
    /// error: the UI shows the degraded state and a launch does nothing.
    pub fn list(&self) -> SinkList {
        let player = self.player().map(str::to_string);
        if !self.runner.available("pactl") {
            return SinkList {
                sinks: Vec::new(),
                default_sink: String::new(),
                pactl: false,
                player,
            };
        }
        let short = self
            .runner
            .run("pactl", &["list".into(), "short".into(), "sinks".into()])
            .unwrap_or_default();
        let long = self
            .runner
            .run("pactl", &["list".into(), "sinks".into()])
            .unwrap_or_default();
        let default = self
            .runner
            .run("pactl", &["get-default-sink".into()])
            .map(|output| parse_default_sink(&output))
            .unwrap_or_default();
        let short = parse_short_sinks(&short);
        let descriptions = parse_sink_descriptions(&long);
        SinkList {
            sinks: compose_sinks(&short, &descriptions, &default),
            default_sink: default,
            pactl: true,
            player,
        }
    }

    /// Every current sink-input, or an empty list when `pactl` is absent.
    pub fn sink_inputs(&self) -> Result<Vec<SinkInput>, String> {
        if !self.runner.available("pactl") {
            return Ok(Vec::new());
        }
        let output = self
            .runner
            .run("pactl", &["list".into(), "sink-inputs".into()])?;
        Ok(parse_sink_inputs(&output))
    }

    /// Move one sink-input; the only mutating call the router makes.
    pub fn move_input(&self, index: u32, sink: &str) -> Result<(), String> {
        self.runner.run(
            "pactl",
            &[
                "move-sink-input".to_string(),
                index.to_string(),
                sink.to_string(),
            ],
        )?;
        Ok(())
    }

    /// Play a short tone on `sink`. Generates the WAV locally: no bundled
    /// sound file, no invented player binary.
    pub fn test_tone(&self, sink: &str) -> Result<String, String> {
        let sink = sink.trim();
        if sink.is_empty() {
            return Err("Choose an audio device first.".to_string());
        }
        let player = self.player().ok_or_else(|| {
            "No audio player was found. Install PipeWire's pw-play or PulseAudio's paplay."
                .to_string()
        })?;
        let path = tone_path();
        let bytes = tone_wav(48_000, 440.0, 260);
        std::fs::write(&path, bytes)
            .map_err(|error| format!("Could not write the test tone: {error}"))?;
        let path = path.to_string_lossy().to_string();
        let args = if player == "pw-play" {
            vec![format!("--target={sink}"), path]
        } else {
            vec![format!("--device={sink}"), path]
        };
        self.runner.run_detached(player, &args)?;
        Ok(player.to_string())
    }

    /// Volume and mute state of the resolved sink.
    pub fn volume_state(&self, requested: Option<&str>, global_default: &str) -> VolumeState {
        let list = self.list();
        if !list.pactl {
            return VolumeState {
                sink: String::new(),
                label: String::new(),
                percent: 0,
                muted: false,
                available: false,
                pactl: false,
            };
        }
        let plan = resolve_plan(requested.unwrap_or_default(), global_default, &list);
        let Some(sink) = plan.sink else {
            return VolumeState {
                sink: String::new(),
                label: String::new(),
                percent: 0,
                muted: false,
                available: false,
                pactl: true,
            };
        };
        let volume = self
            .runner
            .run("pactl", &["get-sink-volume".to_string(), sink.name.clone()])
            .ok()
            .and_then(|output| parse_volume_percent(&output));
        let muted = self
            .runner
            .run("pactl", &["get-sink-mute".to_string(), sink.name.clone()])
            .ok()
            .and_then(|output| parse_mute(&output));
        VolumeState {
            label: sink.label(),
            sink: sink.name,
            percent: volume.unwrap_or(0),
            muted: muted.unwrap_or(false),
            available: volume.is_some(),
            pactl: true,
        }
    }

    /// Apply one volume action and return the new state so the OSD is exact.
    pub fn step_volume(
        &self,
        action: VolumeAction,
        requested: Option<&str>,
        global_default: &str,
    ) -> Result<VolumeState, String> {
        let state = self.volume_state(requested, global_default);
        if !state.available {
            return Err("No audio output is available right now.".to_string());
        }
        self.runner
            .run("pactl", &action.args(&state.sink))
            .map_err(|error| format!("Could not change the volume: {error}"))?;
        Ok(self.volume_state(Some(&state.sink), global_default))
    }
}

/// Percentage of the first channel in `pactl get-sink-volume` output.
pub fn parse_volume_percent(output: &str) -> Option<u8> {
    let line = output
        .lines()
        .find(|line| line.trim_start().starts_with("Volume:"))?;
    let percent = line.find('%')?;
    let digits: String = line[..percent]
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse::<u8>().ok().map(|value| value.min(100))
}

/// `pactl get-sink-mute` → `Some(true)` for `Mute: yes`.
pub fn parse_mute(output: &str) -> Option<bool> {
    let line = output
        .lines()
        .find(|line| line.trim_start().starts_with("Mute:"))?;
    let value = line.split_once(':')?.1.trim();
    Some(value.eq_ignore_ascii_case("yes"))
}

/// Temporary WAV path for the test tone, stable per process so a burst of
/// clicks rewrites one file instead of filling the temp directory.
fn tone_path() -> PathBuf {
    std::env::temp_dir().join(format!("ludex-audio-test-{}.wav", std::process::id()))
}

/// A short mono 16-bit PCM sine wave, written without a bundled asset.
pub fn tone_wav(sample_rate: u32, frequency: f32, millis: u32) -> Vec<u8> {
    let sample_rate = sample_rate.max(8_000);
    let millis = millis.clamp(40, 2_000);
    let samples = (sample_rate as u64 * millis as u64 / 1_000) as u32;
    let data_len = samples * 2;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    let step = frequency.max(20.0) * std::f32::consts::TAU / sample_rate as f32;
    for index in 0..samples {
        let amplitude = (index as f32 * step).sin() * 0.28;
        let sample = (amplitude * i16::MAX as f32) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/* ------------------------------- routing --------------------------------- */

/// Move the game's streams to its sink, bounded.
///
/// Runs on a worker thread after the launch: the game exists before its audio
/// stream does, so the loop re-probes until the stream appears or the window
/// closes. A sink that disappeared is replaced by the profile default and the
/// fallback is emitted once as `audio://notice`.
pub fn spawn_route(app: AppHandle, game_id: i64, root_pid: u32, game_sink: String) {
    std::thread::spawn(move || {
        let global = {
            let state = app.state::<AppState>();
            state.settings_snapshot().audio_default_sink
        };
        let service = AudioService::system();
        let mut sampler = crate::process_tree::ProcessSampler::new();
        let mut announced: Option<String> = None;

        for attempt in 0..ROUTE_MAX_ATTEMPTS {
            let snapshot: Vec<ProcessInfo> = sampler.snapshot();
            let pids = subtree_pids(&snapshot, root_pid);
            if pids.is_empty() && attempt > 2 {
                tracing::debug!(
                    game_id,
                    root_pid,
                    "the game ended before its audio stream appeared"
                );
                return;
            }
            let list = service.list();
            if !list.pactl {
                return;
            }
            let plan = resolve_plan(&game_sink, &global, &list);
            if let Some(notice) = plan.notice {
                let key = format!("{:?}:{}", notice.kind, notice.requested);
                if announced.as_deref() != Some(key.as_str()) {
                    announced = Some(key);
                    let event = AudioNoticeEvent {
                        game_id,
                        kind: notice.kind,
                        requested: notice.requested,
                        used: notice.used,
                    };
                    if let Err(error) = app.emit("audio://notice", event) {
                        tracing::debug!(error = %error, game_id, "could not emit the audio notice");
                    }
                }
            }
            let Some(sink) = plan.sink else {
                return;
            };
            match service.sink_inputs() {
                Ok(inputs) => {
                    let matching = matching_sink_inputs(&inputs, &pids);
                    if !matching.is_empty() {
                        let mut pending = 0;
                        for input in matching {
                            if input.sink_index == Some(sink.index) {
                                continue;
                            }
                            pending += 1;
                            if let Err(error) = service.move_input(input.index, &sink.name) {
                                tracing::warn!(
                                    game_id,
                                    sink_input = input.index,
                                    error = %error,
                                    "could not move the game's audio stream"
                                );
                            }
                        }
                        if pending == 0 {
                            tracing::info!(game_id, sink = %sink.name, "the game's audio follows its profile");
                            return;
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(game_id, error = %error, "could not list the audio streams")
                }
            }
            std::thread::sleep(ROUTE_RETRY_DELAY);
        }
        tracing::debug!(
            game_id,
            "audio routing stopped after the bounded retry window"
        );
    });
}

/* -------------------------------- commands ------------------------------- */

/// Probe the sinks and the Ludex override for the per-game Audio panel.
#[tauri::command]
pub fn audio_sinks(state: tauri::State<'_, AppState>) -> AudioOverview {
    let global = state.settings_snapshot().audio_default_sink;
    AudioOverview::from_list(AudioService::system().list(), &global)
}

/// Play a short tone on one device, through the installed player.
#[tauri::command]
pub fn audio_test_sink(sink: String) -> Result<String, String> {
    AudioService::system().test_tone(&sink)
}

/// The live volume/mute state of a sink; `null` picks the resolved default.
#[tauri::command]
pub fn audio_volume_state(state: tauri::State<'_, AppState>, sink: Option<String>) -> VolumeState {
    let global = state.settings_snapshot().audio_default_sink;
    AudioService::system().volume_state(sink.as_deref(), &global)
}

/// Change one sink's volume or mute and return the new state for the OSD.
#[tauri::command]
pub fn audio_volume_step(
    state: tauri::State<'_, AppState>,
    direction: String,
    sink: Option<String>,
) -> Result<VolumeState, String> {
    let action = VolumeAction::parse(&direction)
        .ok_or_else(|| format!("Unknown volume action: {direction}"))?;
    let global = state.settings_snapshot().audio_default_sink;
    AudioService::system().step_volume(action, sink.as_deref(), &global)
}

/// Store (or clear, with an empty string) the global output override.
#[tauri::command]
pub fn set_audio_default_sink(
    state: tauri::State<'_, AppState>,
    sink: String,
) -> Result<(), String> {
    let sink = sink.trim().to_string();
    if !sink.is_empty() {
        let list = AudioService::system().list();
        if !list.pactl {
            return Err("Audio routing needs PipeWire or PulseAudio on this host.".to_string());
        }
        if !list.sinks.iter().any(|candidate| candidate.name == sink) {
            return Err(format!("The audio device {sink} is not connected."));
        }
    }
    state.update_settings(|settings| {
        settings.audio_default_sink = sink;
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT_SINKS: &str = "\
48\talsa_output.pci-0000_00_1f.3.analog-stereo\tPipeWire\ts32le 2ch 48000Hz\tIDLE
51\talsa_output.pci-0000_01_00.1.hdmi-stereo\tPipeWire\ts32le 2ch 48000Hz\tSUSPENDED
";

    const LONG_SINKS: &str = "\
Sink #48
\tState: SUSPENDED
\tName: alsa_output.pci-0000_00_1f.3.analog-stereo
\tDescription: Built-in Audio Analog Stereo
\tDriver: PipeWire
Sink #51
\tState: SUSPENDED
\tName: alsa_output.pci-0000_01_00.1.hdmi-stereo
\tDescription: HDMI / DisplayPort 1
";

    const SINK_INPUTS: &str = "\
Sink Input #41
\tDriver: PipeWire
\tClient: 62
\tSink: 48
\tProperties:
\t\tapplication.name = \"Proton game\"
\t\tapplication.process.id = \"4242\"
\t\tapplication.process.binary = \"wine64-preloader\"
Sink Input #42
\tSink: 51
\tProperties:
\t\tapplication.name = \"Music\"
\t\tapplication.process.id = \"777\"
Sink Input #43
\tSink: 48
\tProperties:
\t\tapplication.name = \"No pid stream\"
";

    fn list() -> SinkList {
        let short = parse_short_sinks(SHORT_SINKS);
        let descriptions = parse_sink_descriptions(LONG_SINKS);
        SinkList {
            sinks: compose_sinks(
                &short,
                &descriptions,
                "alsa_output.pci-0000_00_1f.3.analog-stereo",
            ),
            default_sink: "alsa_output.pci-0000_00_1f.3.analog-stereo".to_string(),
            pactl: true,
            player: Some("pw-play".to_string()),
        }
    }

    fn pid_tree(pids: &[u32]) -> BTreeSet<u32> {
        pids.iter().copied().collect()
    }

    #[test]
    fn short_sinks_and_descriptions_are_parsed() {
        let short = parse_short_sinks(SHORT_SINKS);
        assert_eq!(short.len(), 2);
        assert_eq!(short[0].0, 48);
        assert_eq!(short[1].1, "alsa_output.pci-0000_01_00.1.hdmi-stereo");

        let descriptions = parse_sink_descriptions(LONG_SINKS);
        let sinks = compose_sinks(
            &short,
            &descriptions,
            "alsa_output.pci-0000_01_00.1.hdmi-stereo",
        );
        assert_eq!(sinks[0].description, "Built-in Audio Analog Stereo");
        assert!(!sinks[0].is_default);
        assert!(sinks[1].is_default);
        assert_eq!(parse_default_sink("alsa_output.hdmi\n"), "alsa_output.hdmi");
        assert_eq!(parse_default_sink("\n"), "");
    }

    #[test]
    fn sink_inputs_match_the_pid_tree() {
        let inputs = parse_sink_inputs(SINK_INPUTS);
        assert_eq!(inputs.len(), 3);
        assert_eq!(
            inputs[0],
            SinkInput {
                index: 41,
                sink_index: Some(48),
                pid: Some(4242)
            }
        );
        assert_eq!(inputs[2].pid, None);

        // Proton spawns the stream from a descendant, not the root PID.
        let matched = matching_sink_inputs(&inputs, &pid_tree(&[4200, 4242, 4243]));
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].index, 41);
        assert!(matching_sink_inputs(&inputs, &pid_tree(&[])).is_empty());
        // A stream from another game on the same machine is never moved.
        assert!(matching_sink_inputs(&inputs, &pid_tree(&[777])).len() == 1);
    }

    #[test]
    fn a_present_game_sink_wins_over_every_default() {
        let list = list();
        let plan = resolve_plan(
            "alsa_output.pci-0000_01_00.1.hdmi-stereo",
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
            &list,
        );
        assert_eq!(
            plan.sink.unwrap().name,
            "alsa_output.pci-0000_01_00.1.hdmi-stereo"
        );
        assert!(plan.notice.is_none());
    }

    #[test]
    fn an_unplugged_game_sink_falls_back_to_the_profile_default() {
        let list = list();
        let plan = resolve_plan(
            "alsa_output.usb-headset.analog-stereo",
            "alsa_output.pci-0000_01_00.1.hdmi-stereo",
            &list,
        );
        let sink = plan.sink.expect("the global override is present");
        assert_eq!(sink.name, "alsa_output.pci-0000_01_00.1.hdmi-stereo");
        let notice = plan.notice.expect("the fallback is reported");
        assert_eq!(notice.kind, NoticeKind::GameSinkMissing);
        assert_eq!(notice.requested, "alsa_output.usb-headset.analog-stereo");
        assert_eq!(notice.used.as_deref(), Some("HDMI / DisplayPort 1"));
    }

    #[test]
    fn an_unplugged_global_default_falls_back_to_the_system_default() {
        let list = list();
        let plan = resolve_plan("", "alsa_output.usb-headset.analog-stereo", &list);
        assert_eq!(
            plan.sink.unwrap().name,
            "alsa_output.pci-0000_00_1f.3.analog-stereo"
        );
        let notice = plan.notice.expect("the fallback is reported");
        assert_eq!(notice.kind, NoticeKind::DefaultSinkMissing);
    }

    #[test]
    fn no_sink_at_all_is_reported_without_a_target() {
        let list = SinkList {
            pactl: true,
            ..SinkList::default()
        };
        let plan = resolve_plan("", "", &list);
        assert!(plan.sink.is_none());
        assert_eq!(plan.notice.unwrap().kind, NoticeKind::NoSink);
    }

    #[test]
    fn the_pactl_command_lines_are_exact() {
        let moved = VolumeAction::Up.args("alsa_output.hdmi");
        assert_eq!(moved, vec!["set-sink-volume", "alsa_output.hdmi", "+5%"]);
        assert_eq!(
            VolumeAction::Down.args("alsa_output.hdmi"),
            vec!["set-sink-volume", "alsa_output.hdmi", "-5%"]
        );
        assert_eq!(
            VolumeAction::ToggleMute.args("alsa_output.hdmi"),
            vec!["set-sink-mute", "alsa_output.hdmi", "toggle"]
        );

        let fake = FakeRunner::new(true);
        let service = AudioService::with_runner(Arc::new(fake.clone()));
        service.move_input(41, "alsa_output.hdmi").unwrap();
        assert_eq!(
            fake.calls(),
            vec!["pactl move-sink-input 41 alsa_output.hdmi".to_string()]
        );
    }

    #[test]
    fn the_bounded_retry_waits_for_a_late_stream() {
        let scripted = std::cell::RefCell::new(vec![
            (Vec::new(), pid_tree(&[10])),
            (
                vec![SinkInput {
                    index: 41,
                    sink_index: Some(48),
                    pid: Some(10),
                }],
                pid_tree(&[10]),
            ),
            (
                vec![SinkInput {
                    index: 41,
                    sink_index: Some(51),
                    pid: Some(10),
                }],
                pid_tree(&[10]),
            ),
        ]);
        let mut moves = Vec::new();
        let report = run_route_loop(
            8,
            51,
            || {
                let mut script = scripted.borrow_mut();
                if script.len() > 1 {
                    Ok(script.remove(0))
                } else {
                    Ok(script[0].clone())
                }
            },
            |index| {
                moves.push(index);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.attempts, 3);
        assert_eq!(moves, vec![41]);
        assert!(report.done);
    }

    #[test]
    fn the_retry_loop_is_bounded() {
        let mut moves = 0;
        let report = run_route_loop(
            4,
            51,
            || {
                Ok((
                    vec![SinkInput {
                        index: 41,
                        sink_index: Some(48),
                        pid: Some(10),
                    }],
                    pid_tree(&[10]),
                ))
            },
            |_| {
                moves += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(report.attempts, 4);
        assert_eq!(moves, 4);
        assert!(!report.done);
    }

    #[test]
    fn volume_and_mute_parsing_reads_the_first_channel() {
        let volume = "Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: 32768 /  50% / -18.06 dB\n        balance 0.00\n";
        assert_eq!(parse_volume_percent(volume), Some(50));
        assert_eq!(
            parse_volume_percent("Volume: front-left: 65536 / 100% / 0.00 dB"),
            Some(100)
        );
        assert_eq!(parse_volume_percent("no volume here"), None);
        assert_eq!(parse_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_mute("Mute: no\n"), Some(false));
        assert_eq!(parse_mute("garbage"), None);
    }

    #[test]
    fn the_test_tone_is_a_short_wav() {
        let bytes = tone_wav(48_000, 440.0, 250);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let data_len = u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
        assert_eq!(bytes.len(), 44 + data_len as usize);
        // 250 ms at 48 kHz, 2 bytes per sample.
        assert_eq!(data_len, 24_000);
        assert!(bytes[44..].iter().any(|byte| *byte != 0));
    }

    #[test]
    fn missing_binaries_degrade_to_an_empty_probe() {
        let fake = FakeRunner::new(false);
        let service = AudioService::with_runner(Arc::new(fake.clone()));
        let list = service.list();
        assert!(!list.pactl);
        assert!(list.sinks.is_empty());
        assert!(list.player.is_none());
        assert!(service.sink_inputs().unwrap().is_empty());
        let state = service.volume_state(None, "");
        assert!(!state.available);
        assert!(!state.pactl);
        let error = service.test_tone("sink").unwrap_err();
        assert!(
            error.contains("pw-play"),
            "error names the players: {error}"
        );
        assert!(error.contains("paplay"), "error names the players: {error}");
        assert!(fake.calls().is_empty());
    }

    #[test]
    fn the_tone_player_is_chosen_by_availability() {
        let fake = FakeRunner::new(true);
        fake.set_available("pw-play", true);
        fake.set_available("paplay", true);
        let service = AudioService::with_runner(Arc::new(fake.clone()));
        assert_eq!(service.player(), Some("pw-play"));
        fake.set_available("pw-play", false);
        assert_eq!(service.player(), Some("paplay"));
    }

    /// A scripted host: `available` is settable, `run` records the exact argv.
    #[derive(Clone)]
    struct FakeRunner {
        available: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl FakeRunner {
        fn new(pactl: bool) -> Self {
            let mut available = std::collections::BTreeSet::new();
            if pactl {
                available.insert("pactl".to_string());
            }
            Self {
                available: Arc::new(std::sync::Mutex::new(available)),
                calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        fn set_available(&self, program: &str, present: bool) {
            if let Ok(mut available) = self.available.lock() {
                if present {
                    available.insert(program.to_string());
                } else {
                    available.remove(program);
                }
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .map(|calls| calls.clone())
                .unwrap_or_default()
        }
    }

    impl AudioRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> Result<String, String> {
            self.calls
                .lock()
                .map_err(|_| "poisoned".to_string())?
                .push(format!("{program} {}", args.join(" ")));
            Ok(String::new())
        }

        fn run_detached(&self, program: &str, args: &[String]) -> Result<(), String> {
            self.calls
                .lock()
                .map_err(|_| "poisoned".to_string())?
                .push(format!("{program} {}", args.join(" ")));
            Ok(())
        }

        fn available(&self, program: &str) -> bool {
            self.available
                .lock()
                .map(|available| available.contains(program))
                .unwrap_or(false)
        }
    }
}
