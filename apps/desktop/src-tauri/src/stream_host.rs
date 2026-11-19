//! Streaming host prerequisites (#291, epic #282): make a Linux host ready for
//! Moonlight without a wiki.
//!
//! Ludex already publishes its library into Sunshine's `apps.json`. This module
//! covers the host side of the same story:
//!
//! - **detect** Sunshine (binary, systemd unit, `sunshine.conf`), the capture
//!   path (KMS nodes, seat/session) and the encoder (`vainfo`, `ffmpeg`), plus
//!   the connected outputs and the interface MTU;
//! - **self-test** with distinct findings for "not installed", "service
//!   disabled", "display blanked", "wrong output captured" and "encoder
//!   unavailable", so the UI can offer the matching fix;
//! - **write only what Ludex depends on** (`output_name`, `stream_audio`,
//!   `packet_size`) with a byte-identical backup, a dry-run plan and a revert;
//! - **open the exact Sunshine port ranges** through ufw, firewalld or nftables
//!   (never a blanket allow), after a polkit prompt that lists every rule, and
//!   close only the rules Ludex added;
//! - **pair** through Sunshine's HTTP API (`/api/pin`, `/api/apps`) with the
//!   admin credentials, or fall back to the local Sunshine page in a window.
//!
//! Safety rules encoded here:
//! - every probe is injected ([`StreamHostEnv`]), so tests classify fixture
//!   trees without a real host, service or firewall;
//! - no write happens without [`StreamConfigPlan`] and an explicit `consent`
//!   flag, and the plan is recomputed inside `apply` instead of trusting the
//!   webview;
//! - a revert only restores bytes that match the recorded SHA-256, and only
//!   removes firewall rules recorded as added by Ludex;
//! - no endpoint is invented: `/api/pin` and `/api/apps` are the two endpoints
//!   upstream Sunshine serves in `src/confighttp.cpp`.
//!
//! Command probes are bounded by [`PROBE_TIMEOUT`]; an elevated firewall or
//! config write is bounded by [`ELEVATED_TIMEOUT`]. Everything runs on a
//! blocking thread.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::remote_play::{parse_sunshine_conf, sunshine_port_base, sunshine_web_ports};
use crate::settings::LocalSettings;
use crate::state::AppState;

/// A single probe command may not take longer than this.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
/// One elevated firewall/config operation (polkit prompt included) may not take
/// longer than this.
pub const ELEVATED_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Moonlight's pairing PIN is four digits.
pub const PIN_LEN: usize = 4;
/// Persisted host state, next to `settings.json`.
pub const STATE_FILE: &str = "stream-host.json";
/// Backup suffix for `sunshine.conf`; a revert restores these bytes.
pub const BACKUP_SUFFIX: &str = ".ludex.bak";
/// Table Ludex owns in nftables, so a revert cannot touch anything else.
pub const NFT_TABLE: &str = "ludex_stream";
/// Secret keys for Sunshine's admin credentials (HTTP Basic).
pub const SECRET_USER: &str = "stream-host.sunshine-user";
pub const SECRET_PASSWORD: &str = "stream-host.sunshine-password";
/// Name Sunshine records for a client paired from Ludex.
pub const PAIR_NAME: &str = "Ludex";
/// Config keys Ludex owns. Nothing else in `sunshine.conf` is touched.
pub const CONFIG_KEYS: [&str; 3] = ["output_name", "stream_audio", "packet_size"];

/// TCP ranges Moonlight needs: HTTPS/NVHTTP, HTTP pairing, Web UI, RTSP.
pub const TCP_RANGES: [(u16, u16); 2] = [(47984, 47990), (48010, 48010)];
/// UDP range: video, control, audio, microphone, RTSP.
pub const UDP_RANGES: [(u16, u16); 1] = [(47998, 48010)];

/* -------------------------------- errors --------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHostError {
    pub code: String,
    pub message: String,
}

impl StreamHostError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StreamHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for StreamHostError {}

/* ------------------------------ probe input ------------------------------ */

/// One bounded command result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub success: bool,
    pub text: String,
}

impl ToolOutput {
    fn new(success: bool, text: impl Into<String>) -> Self {
        Self {
            success,
            text: text.into(),
        }
    }
}

/// Everything a probe reads. `real` fills it from the host; tests fill it from
/// fixture trees and canned command output.
#[derive(Debug, Clone)]
pub struct StreamHostEnv {
    pub supported: bool,
    pub path_dirs: Vec<PathBuf>,
    /// `sunshine.conf` candidates in priority order.
    pub config_candidates: Vec<PathBuf>,
    /// `/sys/class/drm`.
    pub drm_dir: PathBuf,
    /// `/dev/dri`.
    pub dev_dri: PathBuf,
    /// `/sys/class/net`.
    pub net_dir: PathBuf,
    /// `/proc/net/route`.
    pub route_file: PathBuf,
    pub system_unit_dirs: Vec<PathBuf>,
    pub user_unit_dirs: Vec<PathBuf>,
    /// `systemctl is-enabled sunshine.service`.
    pub systemctl_enabled: Option<ToolOutput>,
    /// `systemctl is-active sunshine.service`.
    pub systemctl_active: Option<ToolOutput>,
    /// `systemctl --user is-active sunshine.service`.
    pub systemctl_user_active: Option<ToolOutput>,
    /// `loginctl show-session … -p Id -p Type -p Active -p Seat`.
    pub loginctl: Option<ToolOutput>,
    /// `sunshine --version`.
    pub version: Option<ToolOutput>,
    /// `sunshine --help`.
    pub help: Option<ToolOutput>,
    /// `vainfo`.
    pub vainfo: Option<ToolOutput>,
    /// `ffmpeg -hide_banner -encoders`.
    pub ffmpeg: Option<ToolOutput>,
}

impl StreamHostEnv {
    /// Real host, with the paths the issue names explicitly.
    pub fn real(settings: &LocalSettings) -> Self {
        let supported = cfg!(target_os = "linux");
        let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut config_candidates = Vec::new();
        let override_path = settings.sunshine_conf_path.trim();
        if !override_path.is_empty() {
            config_candidates.push(PathBuf::from(override_path));
        }
        if let Some(home) = &home {
            config_candidates.push(home.join(".config/sunshine/sunshine.conf"));
        }
        config_candidates.push(PathBuf::from("/etc/sunshine/sunshine.conf"));
        let apps_override = settings.sunshine_apps_path.trim();
        if !apps_override.is_empty() {
            let conf = Path::new(apps_override)
                .parent()
                .map(|parent| parent.join("sunshine.conf"));
            if let Some(conf) = conf {
                if !config_candidates.contains(&conf) {
                    config_candidates.push(conf);
                }
            }
        }

        let system_unit_dirs: Vec<PathBuf> = [
            "/etc/systemd/system",
            "/usr/lib/systemd/system",
            "/lib/systemd/system",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let user_unit_dirs: Vec<PathBuf> = home
            .as_ref()
            .map(|home| vec![home.join(".config/systemd/user")])
            .unwrap_or_default();

        let (systemctl_enabled, systemctl_active, systemctl_user_active, loginctl) = if supported {
            let session = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "self".to_string());
            (
                run_tool(&path_dirs, "systemctl", &["is-enabled", "sunshine.service"]),
                run_tool(&path_dirs, "systemctl", &["is-active", "sunshine.service"]),
                run_tool(
                    &path_dirs,
                    "systemctl",
                    &["--user", "is-active", "sunshine.service"],
                ),
                run_tool(
                    &path_dirs,
                    "loginctl",
                    &[
                        "show-session",
                        &session,
                        "-p",
                        "Id",
                        "-p",
                        "Type",
                        "-p",
                        "Active",
                        "-p",
                        "Seat",
                    ],
                ),
            )
        } else {
            (None, None, None, None)
        };
        let (version, help, vainfo, ffmpeg) = if supported {
            (
                run_tool(&path_dirs, "sunshine", &["--version"]),
                run_tool(&path_dirs, "sunshine", &["--help"]),
                run_tool(&path_dirs, "vainfo", &[]),
                run_tool(&path_dirs, "ffmpeg", &["-hide_banner", "-encoders"]),
            )
        } else {
            (None, None, None, None)
        };

        Self {
            supported,
            path_dirs,
            config_candidates,
            drm_dir: PathBuf::from("/sys/class/drm"),
            dev_dri: PathBuf::from("/dev/dri"),
            net_dir: PathBuf::from("/sys/class/net"),
            route_file: PathBuf::from("/proc/net/route"),
            system_unit_dirs,
            user_unit_dirs,
            systemctl_enabled,
            systemctl_active,
            systemctl_user_active,
            loginctl,
            version,
            help,
            vainfo,
            ffmpeg,
        }
    }
}

/* ------------------------------ report types ------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Ok,
    Warning,
    Missing,
    Broken,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCheck {
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamService {
    pub unit: Option<String>,
    pub enabled: bool,
    pub active: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamCapture {
    pub kms: bool,
    pub drm_nodes: usize,
    pub session: Option<String>,
    pub session_type: Option<String>,
    pub seat: Option<String>,
    pub session_active: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayOutput {
    pub name: String,
    pub connected: bool,
    /// DPMS state when the kernel exposes it; `None` when unknown.
    pub active: Option<bool>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkMtu {
    pub interface: String,
    pub mtu: u16,
    pub up: bool,
    pub tunnel: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MtuChoice {
    pub interface: String,
    pub mtu: u16,
    pub packet_size: u16,
    /// `override`, `tunnel`, `route` or `lowest`.
    pub source: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EncoderState {
    pub available: bool,
    pub hardware: bool,
    pub names: Vec<String>,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamHostReport {
    pub supported: bool,
    pub detected: bool,
    pub ready: bool,
    pub binary: Option<String>,
    pub version: Option<String>,
    pub config_path: Option<String>,
    pub config_exists: bool,
    pub configured_output: Option<String>,
    pub configured_audio: Option<String>,
    pub configured_packet_size: Option<u16>,
    pub service: StreamService,
    pub capture: StreamCapture,
    pub outputs: Vec<DisplayOutput>,
    pub interfaces: Vec<NetworkMtu>,
    pub mtu: Option<MtuChoice>,
    pub encoder: EncoderState,
    pub credentials_stored: bool,
    /// True when a previous Ludex run left a revertible `sunshine.conf`.
    pub config_revertible: bool,
    /// True when a previous Ludex run opened firewall rules it can close again.
    pub firewall_revertible: bool,
    pub checks: Vec<StreamCheck>,
    pub warnings: Vec<String>,
    pub message: String,
    pub checked_at: String,
    pub duration_ms: u64,
}

/* -------------------------------- helpers -------------------------------- */

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
}

fn find_binary(env: &StreamHostEnv, name: &str) -> Option<PathBuf> {
    env.path_dirs
        .iter()
        .map(|dir| dir.join(name))
        .chain([PathBuf::from("/usr/bin").join(name)])
        .find(|path| path.is_file())
}

fn run_tool(path_dirs: &[PathBuf], name: &str, args: &[&str]) -> Option<ToolOutput> {
    let program = path_dirs
        .iter()
        .map(|dir| dir.join(name))
        .chain([PathBuf::from("/usr/bin").join(name)])
        .find(|path| path.is_file())?;
    run_bounded(&program, args)
}

fn run_bounded(program: &Path, args: &[&str]) -> Option<ToolOutput> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                let text = if stdout.trim().is_empty() {
                    stderr
                } else {
                    format!("{stdout}\n{stderr}")
                };
                return Some(ToolOutput::new(status.success(), text.trim().to_string()));
            }
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => {
                let _ = child.kill();
                return None;
            }
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = *chunk.get(1).unwrap_or(&0) as u32;
        let third = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (first << 16) | (second << 8) | third;
        out.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/* ------------------------------- outputs --------------------------------- */

/// Enumerate connectors from `/sys/class/drm`, deduplicated by connector name.
pub fn parse_outputs(drm_dir: &Path) -> Vec<DisplayOutput> {
    let Ok(entries) = std::fs::read_dir(drm_dir) else {
        return Vec::new();
    };
    let mut outputs: BTreeMap<String, DisplayOutput> = BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some((_, connector)) = name.split_once('-') else {
            continue;
        };
        if !name.starts_with("card") || connector.is_empty() {
            continue;
        }
        let dir = entry.path();
        let status = read_trimmed(&dir.join("status")).unwrap_or_default();
        let connected = status.eq_ignore_ascii_case("connected");
        let dpms = read_trimmed(&dir.join("dpms")).map(|state| {
            !(state.eq_ignore_ascii_case("off")
                || state.eq_ignore_ascii_case("suspend")
                || state.eq_ignore_ascii_case("doze"))
        });
        let (width, height) = read_trimmed(&dir.join("modes"))
            .and_then(|modes| {
                let line = modes.lines().next()?.trim().to_string();
                let (w, h) = line.split_once('x')?;
                Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))
            })
            .map(|(w, h)| (Some(w), Some(h)))
            .unwrap_or((None, None));
        outputs
            .entry(connector.to_string())
            .or_insert(DisplayOutput {
                name: connector.to_string(),
                connected,
                active: dpms,
                width,
                height,
            });
    }
    outputs.into_values().collect()
}

fn connected_outputs(outputs: &[DisplayOutput]) -> Vec<&DisplayOutput> {
    outputs.iter().filter(|output| output.connected).collect()
}

/// Pick the output Sunshine should capture: an explicit choice, the pinned
/// output when it is still connected, then the only connected output.
pub fn choose_output(
    outputs: &[DisplayOutput],
    configured: Option<&str>,
    requested: Option<&str>,
) -> Result<String, String> {
    let connected = connected_outputs(outputs);
    if connected.is_empty() {
        return Err(
            "No display output is connected. Plug in the monitor or enable the virtual display, then rescan."
                .to_string(),
        );
    }
    if let Some(requested) = requested.map(str::trim).filter(|name| !name.is_empty()) {
        if connected.iter().any(|output| output.name == requested) {
            return Ok(requested.to_string());
        }
        return Err(format!(
            "{requested} is not connected. Choose one of: {}.",
            connected
                .iter()
                .map(|output| output.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(configured) = configured.map(str::trim).filter(|name| !name.is_empty()) {
        if connected.iter().any(|output| output.name == configured) {
            return Ok(configured.to_string());
        }
    }
    if connected.len() == 1 {
        return Ok(connected[0].name.clone());
    }
    Err(format!(
        "{} displays are connected: choose the one Sunshine should capture.",
        connected.len()
    ))
}

/* --------------------------------- MTU ----------------------------------- */

/// Payload size for Sunshine's `packet_size`, from the interface MTU.
///
/// 80 bytes are reserved for IPv6, UDP and Sunshine's RTP header: a 1280-byte
/// WireGuard/NetBird tunnel therefore gets a 1200-byte payload. The result is
/// clamped to Sunshine's useful range (1024..=1420) so a jumbo or tiny MTU
/// cannot produce a nonsensical value.
pub fn packet_size_for_mtu(mtu: u16) -> u16 {
    if mtu == 0 {
        return 1024;
    }
    mtu.saturating_sub(80).clamp(1024, 1420)
}

fn tunnel_interface(name: &str) -> bool {
    const PREFIXES: [&str; 8] = [
        "wg",
        "tailscale",
        "netbird",
        "zt",
        "tun",
        "utun",
        "nebula",
        "ipsec",
    ];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn parse_interfaces(net_dir: &Path) -> Vec<NetworkMtu> {
    let Ok(entries) = std::fs::read_dir(net_dir) else {
        return Vec::new();
    };
    let mut interfaces = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "lo" {
            continue;
        }
        let Some(mtu) =
            read_trimmed(&entry.path().join("mtu")).and_then(|text| text.parse::<u16>().ok())
        else {
            continue;
        };
        let state = read_trimmed(&entry.path().join("operstate")).unwrap_or_default();
        interfaces.push(NetworkMtu {
            tunnel: tunnel_interface(&name),
            up: !state.eq_ignore_ascii_case("down"),
            interface: name,
            mtu,
        });
    }
    interfaces.sort_by(|left, right| left.interface.cmp(&right.interface));
    interfaces
}

/// Interface of the IPv4 default route, from `/proc/net/route`.
pub fn parse_default_route(text: &str) -> Option<String> {
    for line in text.lines().skip(1) {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if columns.len() < 2 || columns[1] != "00000000" {
            continue;
        }
        if columns[0] != "lo" {
            return Some(columns[0].to_string());
        }
    }
    None
}

/// Choose the interface whose MTU governs the Moonlight stream.
///
/// An explicit choice wins; then an up tunnel (WireGuard, NetBird, Tailscale…),
/// because that is the path the stream takes; then the default route; then the
/// up interface with the lowest MTU (the most constrained path).
pub fn choose_mtu(
    interfaces: &[NetworkMtu],
    default_route: Option<&str>,
    requested: Option<&str>,
) -> Option<MtuChoice> {
    let pick = |item: &NetworkMtu, source: &str| MtuChoice {
        interface: item.interface.clone(),
        mtu: item.mtu,
        packet_size: packet_size_for_mtu(item.mtu),
        source: source.to_string(),
        detail: format!(
            "{} has an MTU of {}; Sunshine sends {} bytes per packet.",
            item.interface,
            item.mtu,
            packet_size_for_mtu(item.mtu)
        ),
    };

    if let Some(name) = requested.map(str::trim).filter(|name| !name.is_empty()) {
        if let Some(item) = interfaces
            .iter()
            .find(|item| item.interface == name && item.up)
        {
            return Some(pick(item, "override"));
        }
    }
    if let Some(item) = interfaces
        .iter()
        .filter(|item| item.up && item.tunnel)
        .min_by_key(|item| item.mtu)
    {
        return Some(pick(item, "tunnel"));
    }
    if let Some(name) = default_route {
        if let Some(item) = interfaces
            .iter()
            .find(|item| item.up && item.interface == name)
        {
            return Some(pick(item, "route"));
        }
    }
    interfaces
        .iter()
        .filter(|item| item.up)
        .min_by_key(|item| item.mtu)
        .map(|item| pick(item, "lowest"))
}

/* ------------------------------- detection ------------------------------- */

fn parse_encoders(vainfo: Option<&ToolOutput>, ffmpeg: Option<&ToolOutput>) -> EncoderState {
    const NAMES: [&str; 17] = [
        "h264_vaapi",
        "hevc_vaapi",
        "av1_vaapi",
        "vp9_vaapi",
        "h264_nvenc",
        "hevc_nvenc",
        "av1_nvenc",
        "h264_amf",
        "hevc_amf",
        "av1_amf",
        "h264_qsv",
        "hevc_qsv",
        "av1_qsv",
        "h264_vulkan",
        "hevc_vulkan",
        "libx264",
        "libx265",
    ];
    let mut names = BTreeSet::new();
    if let Some(vainfo) = vainfo.filter(|output| output.success) {
        for line in vainfo.text.lines() {
            for codec in ["H264", "HEVC", "AV1", "VP9"] {
                if line.contains(&format!("VAProfile{codec}")) {
                    names.insert(codec.to_ascii_lowercase());
                }
            }
        }
    }
    if let Some(ffmpeg) = ffmpeg {
        for line in ffmpeg.text.lines() {
            for name in NAMES {
                if line.contains(name) {
                    names.insert(name.to_string());
                }
            }
        }
    }
    let names: Vec<String> = names.into_iter().collect();
    let hardware = names.iter().any(|name| !name.starts_with("lib"));
    let available = !names.is_empty();
    let detail = if !available {
        "No H.264, HEVC or AV1 encoder was found. Install the VA-API drivers (vainfo) or a working ffmpeg build.".to_string()
    } else if hardware {
        format!("Encoding available: {}.", names.join(", "))
    } else {
        format!(
            "Only software encoding is available ({}). Latency will be high; install VA-API for hardware encoding.",
            names.join(", ")
        )
    };
    EncoderState {
        available,
        hardware,
        names,
        detail,
    }
}

fn parse_loginctl(output: Option<&ToolOutput>) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    if let Some(output) = output.filter(|output| output.success) {
        for line in output.text.lines() {
            if let Some((key, value)) = line.split_once('=') {
                values.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    values
}

fn find_config(env: &StreamHostEnv) -> Option<PathBuf> {
    if let Some(existing) = env.config_candidates.iter().find(|path| path.is_file()) {
        return Some(existing.clone());
    }
    // Nothing to read: a distro package with a system unit reads /etc, while a
    // per-user install reads ~/.config. Writing to the wrong one changes
    // nothing, so prefer the path the installed unit implies.
    let system_unit = env
        .system_unit_dirs
        .iter()
        .any(|dir| dir.join("sunshine.service").is_file());
    let user_unit = env
        .user_unit_dirs
        .iter()
        .any(|dir| dir.join("sunshine.service").is_file());
    if system_unit && !user_unit {
        if let Some(system) = env
            .config_candidates
            .iter()
            .find(|path| path.starts_with("/etc"))
        {
            return Some(system.clone());
        }
    }
    env.config_candidates.first().cloned()
}

/// Classify the host once.
pub fn detect(settings: &LocalSettings) -> StreamHostReport {
    detect_with(&StreamHostEnv::real(settings))
}

/// Classify probes against an explicit environment.
pub fn detect_with(env: &StreamHostEnv) -> StreamHostReport {
    let started = Instant::now();
    let mut warnings = Vec::new();

    let binary = find_binary(env, "sunshine").map(|path| path.to_string_lossy().to_string());
    let version_text = env
        .version
        .as_ref()
        .filter(|output| output.success)
        .or_else(|| env.help.as_ref().filter(|output| output.success))
        .map(|output| {
            output
                .text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty());
    let config_path = find_config(env);
    let config_text = config_path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok());
    let config_exists = config_path.as_ref().is_some_and(|path| path.is_file());
    let conf: BTreeMap<String, String> = config_text
        .as_deref()
        .map(parse_sunshine_conf)
        .unwrap_or_default();

    let unit_path = env
        .user_unit_dirs
        .iter()
        .chain(env.system_unit_dirs.iter())
        .map(|dir| dir.join("sunshine.service"))
        .find(|path| path.is_file());
    let enabled_raw = env
        .systemctl_enabled
        .as_ref()
        .filter(|output| output.success)
        .map(|output| output.text.trim().to_string());
    let enabled = enabled_raw
        .as_deref()
        .is_some_and(|state| matches!(state, "enabled" | "enabled-runtime" | "static"));
    let active = env
        .systemctl_active
        .as_ref()
        .is_some_and(|output| output.success && output.text.trim() == "active")
        || env
            .systemctl_user_active
            .as_ref()
            .is_some_and(|output| output.success && output.text.trim() == "active");
    let service = StreamService {
        unit: unit_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        enabled,
        active,
        detail: if active && enabled {
            "Sunshine is running and starts at boot.".to_string()
        } else if active {
            "Sunshine is running but not enabled; it will not start after a reboot.".to_string()
        } else if enabled {
            "Sunshine is enabled but not running. Start it before pairing.".to_string()
        } else if unit_path.is_some() {
            "Sunshine has a unit but it is neither enabled nor active.".to_string()
        } else {
            "No sunshine.service unit was found for this user or system.".to_string()
        },
    };
    if unit_path.is_some() && !enabled && !active {
        warnings.push(
            "The Sunshine service is installed but disabled: Moonlight cannot connect until it runs."
                .to_string(),
        );
    }

    let drm_nodes = std::fs::read_dir(&env.dev_dri)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("card"))
                .count()
        })
        .unwrap_or(0);
    let session = parse_loginctl(env.loginctl.as_ref());
    let session_active = session
        .get("Active")
        .map(|value| value.eq_ignore_ascii_case("yes"));
    let capture = StreamCapture {
        kms: drm_nodes > 0,
        drm_nodes,
        session: session.get("Id").cloned(),
        session_type: session.get("Type").cloned(),
        seat: session.get("Seat").cloned(),
        session_active,
        detail: if drm_nodes == 0 {
            "No /dev/dri render node: Sunshine cannot capture the screen. Install the GPU driver."
                .to_string()
        } else if session_active == Some(false) {
            "The graphical session is not active; a switched-away VT is not captured.".to_string()
        } else {
            format!(
                "{} DRM card(s) visible, {} session.",
                drm_nodes,
                session.get("Type").map(String::as_str).unwrap_or("unknown")
            )
        },
    };

    let outputs = parse_outputs(&env.drm_dir);
    let connected = connected_outputs(&outputs);
    let configured_output = conf
        .get("output_name")
        .cloned()
        .filter(|value| !value.is_empty());
    let (_, configured_audio, configured_packet_size) =
        config_view(config_text.as_deref().unwrap_or_default());

    let interfaces = parse_interfaces(&env.net_dir);
    let route_text = std::fs::read_to_string(&env.route_file).unwrap_or_default();
    let default_route = parse_default_route(&route_text);
    let mtu = choose_mtu(&interfaces, default_route.as_deref(), None);
    if let Some(mtu) = &mtu {
        if mtu.mtu < 1280 {
            warnings.push(format!(
                "{} has an MTU of {}; Sunshine packets are capped at {} bytes.",
                mtu.interface, mtu.mtu, mtu.packet_size
            ));
        }
    }

    let encoder = parse_encoders(env.vainfo.as_ref(), env.ffmpeg.as_ref());
    let base_port = sunshine_port_base(&conf);
    if base_port != 47989 {
        warnings.push(format!(
            "Sunshine is configured on port base {base_port}; Ludex opens the default 47984-48010 ranges only."
        ));
    }

    let mut checks = Vec::new();
    checks.push(StreamCheck {
        id: "sunshine".to_string(),
        label: "Sunshine".to_string(),
        status: if binary.is_some() {
            CheckStatus::Ok
        } else {
            CheckStatus::Missing
        },
        detail: match (&binary, &version_text) {
            (Some(path), Some(version)) => format!("{path} ({version})"),
            (Some(path), None) => path.clone(),
            (None, _) => "The sunshine binary is not on PATH.".to_string(),
        },
        hint: if binary.is_none() {
            Some("Install Sunshine from your distribution or the official package.".to_string())
        } else {
            None
        },
    });
    checks.push(StreamCheck {
        id: "service".to_string(),
        label: "sunshine.service".to_string(),
        status: if active && enabled {
            CheckStatus::Ok
        } else if active || enabled {
            CheckStatus::Warning
        } else if unit_path.is_some() {
            CheckStatus::Broken
        } else {
            CheckStatus::Missing
        },
        detail: service.detail.clone(),
        hint: if active && enabled {
            None
        } else {
            Some("Run: systemctl enable --now sunshine".to_string())
        },
    });
    checks.push(StreamCheck {
        id: "capture".to_string(),
        label: "Capture".to_string(),
        status: if !capture.kms {
            CheckStatus::Broken
        } else if capture.session_active == Some(false) {
            CheckStatus::Warning
        } else {
            CheckStatus::Ok
        },
        detail: capture.detail.clone(),
        hint: if capture.kms {
            None
        } else {
            Some("Install the GPU driver so /dev/dri/card* exists.".to_string())
        },
    });
    checks.push(StreamCheck {
        id: "encoder".to_string(),
        label: "Encoder".to_string(),
        status: if !encoder.available {
            CheckStatus::Broken
        } else if !encoder.hardware {
            CheckStatus::Warning
        } else {
            CheckStatus::Ok
        },
        detail: encoder.detail.clone(),
        hint: if encoder.hardware {
            None
        } else {
            Some("Install the VA-API driver package and vainfo.".to_string())
        },
    });
    let output_status = if connected.is_empty()
        || configured_output
            .as_deref()
            .is_some_and(|name| !connected.iter().any(|output| output.name == name))
    {
        CheckStatus::Broken
    } else {
        CheckStatus::Ok
    };
    checks.push(StreamCheck {
        id: "output".to_string(),
        label: "Display output".to_string(),
        status: output_status,
        detail: if connected.is_empty() {
            "No connected display output was found.".to_string()
        } else if output_status == CheckStatus::Broken {
            format!(
                "Sunshine captures {}, which is not connected. Connected: {}.",
                configured_output.clone().unwrap_or_default(),
                connected
                    .iter()
                    .map(|output| output.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            format!(
                "Connected: {}.",
                connected
                    .iter()
                    .map(|output| output.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
        hint: if output_status == CheckStatus::Ok {
            None
        } else {
            Some("Choose the connected output in the streaming wizard.".to_string())
        },
    });

    let ready = checks
        .iter()
        .all(|check| !matches!(check.status, CheckStatus::Missing | CheckStatus::Broken));
    let detected = binary.is_some() || service.unit.is_some();
    let message = if !env.supported {
        "Streaming host checks only apply to Linux.".to_string()
    } else if !detected {
        "Sunshine is not installed on this host.".to_string()
    } else if ready {
        "This host is ready for a Moonlight stream.".to_string()
    } else {
        "Some prerequisites are missing; see the checks below.".to_string()
    };

    StreamHostReport {
        supported: env.supported,
        detected,
        ready,
        binary,
        version: version_text,
        config_path: config_path.map(|path| path.to_string_lossy().to_string()),
        config_exists,
        configured_output,
        configured_audio,
        configured_packet_size,
        service,
        capture,
        outputs,
        interfaces,
        mtu,
        encoder,
        credentials_stored: false,
        config_revertible: false,
        firewall_revertible: false,
        checks,
        warnings,
        message,
        checked_at: crate::util::now_rfc3339(),
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/* ------------------------------- self-test ------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTest {
    pub ready: bool,
    pub findings: Vec<StreamCheck>,
    pub message: String,
}

/// Distinct self-test findings, in the order the user should fix them.
pub const SELF_TEST_CODES: [&str; 8] = [
    "sunshine-missing",
    "sunshine-disabled",
    "capture-unavailable",
    "no-output",
    "display-blanked",
    "wrong-output",
    "encoder-unavailable",
    "mtu-low",
];

/// The message each finding carries. Kept in one table so the UI and the tests
/// cannot drift apart.
pub fn self_test_message(code: &str) -> &'static str {
    match code {
        "sunshine-missing" => {
            "Sunshine is not installed. Install it, then run the self-test again."
        }
        "sunshine-disabled" => {
            "Sunshine is installed but its service is disabled or stopped. Enable it, then run the self-test again."
        }
        "capture-unavailable" => {
            "No capture device: /dev/dri has no card. The GPU driver is missing or the host is headless."
        }
        "no-output" => {
            "No display output is connected. Sunshine has nothing to capture."
        }
        "display-blanked" => {
            "The display is blanked (DPMS off or the session is inactive). Wake it and disable screen blanking while streaming."
        }
        "wrong-output" => {
            "Sunshine is configured to capture an output that is not connected. Pick a connected output in the wizard."
        }
        "encoder-unavailable" => {
            "No H.264, HEVC or AV1 encoder is available. Install the VA-API driver and vainfo."
        }
        "mtu-low" => {
            "The stream interface has an MTU below 1280. Sunshine packets are reduced, but video quality may suffer."
        }
        _ => "Unknown streaming host finding.",
    }
}

fn finding(code: &str, status: CheckStatus) -> StreamCheck {
    StreamCheck {
        id: code.to_string(),
        label: self_test_message(code).to_string(),
        status,
        detail: self_test_message(code).to_string(),
        hint: None,
    }
}

/// Map a detection report to the distinct messages the self-test shows.
pub fn self_test(report: &StreamHostReport) -> SelfTest {
    if !report.supported {
        return SelfTest {
            ready: false,
            findings: Vec::new(),
            message: "Streaming host checks only apply to Linux.".to_string(),
        };
    }
    let mut findings = Vec::new();
    if !report.detected {
        findings.push(finding("sunshine-missing", CheckStatus::Missing));
    } else {
        if !report.service.enabled || !report.service.active {
            findings.push(finding("sunshine-disabled", CheckStatus::Broken));
        }
        if !report.capture.kms {
            findings.push(finding("capture-unavailable", CheckStatus::Broken));
        }
        let connected = connected_outputs(&report.outputs);
        if connected.is_empty() {
            findings.push(finding("no-output", CheckStatus::Broken));
        } else {
            let blanked = report.capture.session_active == Some(false)
                || connected.iter().all(|output| output.active == Some(false));
            if blanked {
                findings.push(finding("display-blanked", CheckStatus::Broken));
            }
            if report
                .configured_output
                .as_deref()
                .is_some_and(|name| !connected.iter().any(|output| output.name == name))
            {
                findings.push(finding("wrong-output", CheckStatus::Broken));
            }
        }
        if !report.encoder.available {
            findings.push(finding("encoder-unavailable", CheckStatus::Broken));
        } else if !report.encoder.hardware {
            findings.push(finding("encoder-unavailable", CheckStatus::Warning));
        }
        if report.mtu.as_ref().is_some_and(|mtu| mtu.mtu < 1280) {
            findings.push(finding("mtu-low", CheckStatus::Warning));
        }
    }
    debug_assert!(SELF_TEST_CODES
        .iter()
        .all(|code| !self_test_message(code).starts_with("Unknown")));
    let ready = findings.is_empty();
    SelfTest {
        ready,
        message: if ready {
            "Self-test passed: Sunshine can capture a display and encode a stream.".to_string()
        } else {
            format!(
                "{} finding(s) need attention before Moonlight can stream.",
                findings.len()
            )
        },
        findings,
    }
}

/* ---------------------------- config: merging ---------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEdit {
    pub key: String,
    pub value: String,
    pub previous: Option<String>,
}

fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn strip_line_ending(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// Merge the three keys Ludex owns into `sunshine.conf`, leaving every other
/// byte alone: comments, ordering, spacing and line endings are preserved.
///
/// A key that appears more than once is edited at its last occurrence, which is
/// the one Sunshine's own parser keeps.
pub fn merge_config(text: &str, writes: &[(String, String)]) -> (String, Vec<ConfigEdit>) {
    let eol = line_ending(text);
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let mut edits = Vec::new();
    for (key, value) in writes {
        let wanted = key.to_ascii_lowercase();
        let mut matched = None;
        for (index, line) in lines.iter().enumerate() {
            let (body, _) = strip_line_ending(line);
            let trimmed = body.trim_start();
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }
            let Some((found, rest)) = body.split_once('=') else {
                continue;
            };
            if found.trim().eq_ignore_ascii_case(&wanted) {
                matched = Some((index, found.trim_end().to_string(), rest.trim().to_string()));
            }
        }
        if let Some((index, prefix, previous)) = matched {
            lines[index] = format!("{prefix} = {value}{eol}");
            edits.push(ConfigEdit {
                key: key.clone(),
                value: value.clone(),
                previous: Some(previous),
            });
        } else {
            edits.push(ConfigEdit {
                key: key.clone(),
                value: value.clone(),
                previous: None,
            });
        }
    }
    // Append missing keys in the order they were requested.
    let missing: Vec<&ConfigEdit> = edits
        .iter()
        .filter(|edit| edit.previous.is_none())
        .collect();
    if !missing.is_empty() {
        if let Some(last) = lines.last_mut() {
            if !last.ends_with('\n') {
                last.push_str(eol);
            }
        }
        for edit in missing {
            lines.push(format!("{} = {}{eol}", edit.key, edit.value));
        }
    }
    (lines.concat(), edits)
}

/// Current values of the keys Ludex depends on.
pub fn config_view(text: &str) -> (Option<String>, Option<String>, Option<u16>) {
    let conf = parse_sunshine_conf(text);
    (
        conf.get("output_name")
            .cloned()
            .filter(|value| !value.is_empty()),
        conf.get("stream_audio").cloned(),
        conf.get("packet_size")
            .and_then(|value| value.parse::<u16>().ok()),
    )
}

/* ----------------------------- config: plan ------------------------------ */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub path: String,
    /// `write`, `backup` or `delete`.
    pub action: String,
    pub bytes: usize,
    pub mode: String,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamConfigPlan {
    pub path: String,
    pub exists: bool,
    pub output: String,
    pub packet_size: u16,
    pub interface: String,
    pub edits: Vec<ConfigEdit>,
    pub files: Vec<PlannedFile>,
    pub commands: Vec<String>,
    pub backup: String,
    pub needs_elevation: bool,
    pub applicable: bool,
    pub blocked: Option<String>,
    pub warnings: Vec<String>,
    pub message: String,
}

fn backup_path_for(config: &Path) -> PathBuf {
    let mut name = config
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "sunshine.conf".to_string());
    name.push_str(BACKUP_SUFFIX);
    config.with_file_name(name)
}

fn config_writable(path: &Path) -> bool {
    if path.is_file() {
        return std::fs::OpenOptions::new().append(true).open(path).is_ok();
    }
    // Walk up to the nearest existing ancestor: a config path may point into a
    // Sunshine folder Ludex will create.
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir.is_dir() {
            let probe = dir.join(".ludex-write-probe");
            let writable = std::fs::write(&probe, b"").is_ok();
            let _ = std::fs::remove_file(&probe);
            return writable;
        }
        ancestor = dir.parent();
    }
    false
}

fn file_mode(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|meta| format!("{:04o}", meta.permissions().mode() & 0o777))
            .unwrap_or_else(|| "0644".to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        "0644".to_string()
    }
}

fn matching_mtu(report: &StreamHostReport, requested_interface: Option<&str>) -> Option<MtuChoice> {
    if let Some(name) = requested_interface.filter(|name| !name.is_empty()) {
        if let Some(item) = report
            .interfaces
            .iter()
            .find(|item| item.up && item.interface == name)
        {
            return Some(MtuChoice {
                interface: item.interface.clone(),
                mtu: item.mtu,
                packet_size: packet_size_for_mtu(item.mtu),
                source: "override".to_string(),
                detail: format!(
                    "{} has an MTU of {}; Sunshine sends {} bytes per packet.",
                    item.interface,
                    item.mtu,
                    packet_size_for_mtu(item.mtu)
                ),
            });
        }
    }
    report.mtu.clone()
}

/// Build the dry-run plan for `sunshine.conf`. Nothing is written.
pub fn plan_config(
    report: &StreamHostReport,
    requested_output: Option<&str>,
    requested_interface: Option<&str>,
) -> Result<StreamConfigPlan, StreamHostError> {
    let Some(path) = report.config_path.as_deref().map(PathBuf::from) else {
        return Err(StreamHostError::new(
            "no_config",
            "No sunshine.conf path is known. Install Sunshine or set its config path first.",
        ));
    };
    let mut blocked = if !report.supported {
        Some("Streaming host prerequisites only apply to Linux.".to_string())
    } else if !report.detected {
        Some("Sunshine is not installed; there is nothing to configure.".to_string())
    } else {
        None
    };
    let output = match choose_output(
        &report.outputs,
        report.configured_output.as_deref(),
        requested_output,
    ) {
        Ok(output) => Some(output),
        Err(reason) => {
            if blocked.is_none() {
                blocked = Some(reason);
            }
            None
        }
    };
    let choice = matching_mtu(report, requested_interface);
    let packet_size = choice
        .as_ref()
        .map(|choice| choice.packet_size)
        .unwrap_or(1024);
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let exists = path.is_file();
    let output_name = output.clone().unwrap_or_default();
    let writes = vec![
        ("output_name".to_string(), output_name.clone()),
        ("stream_audio".to_string(), "on".to_string()),
        ("packet_size".to_string(), packet_size.to_string()),
    ];
    debug_assert_eq!(
        writes
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        CONFIG_KEYS,
        "Ludex must not write sunshine.conf keys outside CONFIG_KEYS"
    );
    let (contents, edits) = merge_config(&original, &writes);
    let backup = backup_path_for(&path);
    let mode = file_mode(&path);
    let changed = contents != original;
    let needs_elevation = !config_writable(&path);
    let mut warnings = Vec::new();
    if let Some(capture) = report
        .configured_output
        .as_deref()
        .filter(|capture| *capture != output_name)
    {
        warnings.push(format!(
            "Sunshine currently captures {capture}; the plan pins {output_name}."
        ));
    }
    let mut files = Vec::new();
    if exists {
        files.push(PlannedFile {
            path: backup.to_string_lossy().to_string(),
            action: "backup".to_string(),
            bytes: original.len(),
            mode: "0644".to_string(),
            preview: format!("Byte-identical copy of {}", path.display()),
        });
    }
    files.push(PlannedFile {
        path: path.to_string_lossy().to_string(),
        action: "write".to_string(),
        bytes: contents.len(),
        mode: mode.clone(),
        preview: contents.clone(),
    });
    let mut commands = vec![format!(
        "cp --preserve=mode {} {}",
        crate::console_session::shell_quote(&path.display().to_string()),
        crate::console_session::shell_quote(&backup.display().to_string())
    )];
    commands.push(format!(
        "install -m {mode} <staged sunshine.conf> {}",
        crate::console_session::shell_quote(&path.display().to_string())
    ));
    if needs_elevation {
        commands.push("pkexec sh -c '<the commands above>' (polkit prompt)".to_string());
    }
    let edit_count = edits.len();
    Ok(StreamConfigPlan {
        path: path.to_string_lossy().to_string(),
        exists,
        output: output_name,
        packet_size,
        interface: choice
            .as_ref()
            .map(|choice| choice.interface.clone())
            .unwrap_or_default(),
        edits,
        files,
        commands,
        backup: backup.to_string_lossy().to_string(),
        needs_elevation,
        applicable: blocked.is_none() && changed,
        blocked: blocked.clone(),
        warnings,
        message: if blocked.is_some() {
            "The plan cannot be applied yet.".to_string()
        } else if !changed {
            "sunshine.conf already has these values; nothing will be written.".to_string()
        } else {
            format!("Ready to write {edit_count} change(s) to sunshine.conf.")
        },
    })
}

/* ------------------------------ persisted state --------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigState {
    pub path: String,
    pub backup: String,
    pub existed: bool,
    pub sha_before: String,
    pub applied_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FirewallState {
    /// `ufw`, `firewalld` or `nftables`.
    pub tool: String,
    /// Rule ids added by Ludex, e.g. `tcp:47984-47990`.
    pub added: Vec<String>,
    pub created_table: bool,
    pub applied_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StreamHostState {
    pub config: Option<ConfigState>,
    pub firewall: Option<FirewallState>,
}

pub fn state_path(app_data: &Path) -> PathBuf {
    app_data.join(STATE_FILE)
}

pub fn read_state(app_data: &Path) -> StreamHostState {
    std::fs::read_to_string(state_path(app_data))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_state(app_data: &Path, state: &StreamHostState) -> Result<(), StreamHostError> {
    let path = state_path(app_data);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            StreamHostError::new(
                "state_failed",
                format!("Could not create {}: {error}", parent.display()),
            )
        })?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|error| {
        StreamHostError::new(
            "state_failed",
            format!("Could not encode host state: {error}"),
        )
    })?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text).map_err(|error| {
        StreamHostError::new(
            "state_failed",
            format!("Could not write {}: {error}", temporary.display()),
        )
    })?;
    std::fs::rename(&temporary, &path).map_err(|error| {
        StreamHostError::new(
            "state_failed",
            format!("Could not replace {}: {error}", path.display()),
        )
    })
}

/* ----------------------------- elevated writes ---------------------------- */

/// Run one shell script through `pkexec`, bounded by [`ELEVATED_TIMEOUT`].
fn run_elevated(script: &str) -> Result<String, StreamHostError> {
    if !crate::paths::find_on_path("pkexec").is_some_and(|path| path.is_file()) {
        return Err(StreamHostError::new(
            "elevation_unavailable",
            "Ludex needs pkexec (polkit) for this change. Install polkit and try again.",
        ));
    }
    let mut child = Command::new("pkexec")
        .args(["sh", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            StreamHostError::new(
                "elevation_failed",
                format!("Could not start pkexec: {error}"),
            )
        })?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stdout.take() {
                    let _ = pipe.read_to_string(&mut stdout);
                }
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                if status.success() {
                    return Ok(stdout);
                }
                return Err(match status.code() {
                    Some(126) => StreamHostError::new(
                        "elevation_cancelled",
                        "The administrator prompt was cancelled; nothing was changed.",
                    ),
                    Some(127) => StreamHostError::new(
                        "elevation_denied",
                        "Polkit refused the request. Your account is not allowed to change this setting.",
                    ),
                    Some(code) => StreamHostError::new(
                        "elevation_failed",
                        format!(
                            "The elevated command failed with exit code {code}: {}",
                            stderr.trim()
                        ),
                    ),
                    None => StreamHostError::new(
                        "elevation_failed",
                        "The elevated command was terminated by a signal.",
                    ),
                });
            }
            Ok(None) if started.elapsed() < ELEVATED_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(StreamHostError::new(
                    "elevation_timeout",
                    "The administrator prompt timed out; nothing was changed.",
                ));
            }
            Err(error) => {
                let _ = child.kill();
                return Err(StreamHostError::new(
                    "elevation_failed",
                    format!("Could not wait for pkexec: {error}"),
                ));
            }
        }
    }
}

/// Stage bytes in a private temporary file and install them through one polkit
/// prompt.
fn stage_and_install(path: &Path, bytes: &[u8], mode: &str) -> Result<(), StreamHostError> {
    let staged = std::env::temp_dir().join(format!(
        "ludex-sunshine-{}.conf",
        crate::util::short_id(&uuid::Uuid::new_v4().to_string())
    ));
    std::fs::write(&staged, bytes).map_err(|error| {
        StreamHostError::new(
            "write_failed",
            format!("Could not stage the new contents: {error}"),
        )
    })?;
    let script = format!(
        "set -eu\nmkdir -p {}\ninstall -m {mode} {} {}\n",
        crate::console_session::shell_quote(
            &path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_default()
        ),
        crate::console_session::shell_quote(&staged.display().to_string()),
        crate::console_session::shell_quote(&path.display().to_string())
    );
    let result = run_elevated(&script);
    let _ = std::fs::remove_file(&staged);
    result.map(|_| ())
}

/// Write bytes to a path, directly when allowed and through polkit otherwise.
fn write_config_bytes(path: &Path, bytes: &[u8], mode: &str) -> Result<(), StreamHostError> {
    if config_writable(path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                StreamHostError::new(
                    "write_failed",
                    format!("Could not create {}: {error}", parent.display()),
                )
            })?;
        }
        // Write next to the target, then rename: an interrupted write never
        // leaves a half-written sunshine.conf.
        let temporary = path.with_file_name(format!(
            "{}.ludex-tmp",
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "sunshine.conf".to_string())
        ));
        std::fs::write(&temporary, bytes).map_err(|error| {
            StreamHostError::new(
                "write_failed",
                format!("Could not write {}: {error}", temporary.display()),
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let parsed = u32::from_str_radix(mode, 8).unwrap_or(0o644);
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(parsed));
        }
        std::fs::rename(&temporary, path).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            StreamHostError::new(
                "write_failed",
                format!("Could not replace {}: {error}", path.display()),
            )
        })?;
        return Ok(());
    }
    stage_and_install(path, bytes, mode)
}

fn remove_config_file(path: &Path) -> Result<(), StreamHostError> {
    if !path.exists() {
        return Ok(());
    }
    if std::fs::remove_file(path).is_ok() {
        return Ok(());
    }
    let script = format!(
        "set -eu\nrm -f {}\n",
        crate::console_session::shell_quote(&path.display().to_string())
    );
    run_elevated(&script).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigApply {
    pub path: String,
    pub backup: String,
    pub written: bool,
    pub sha256: String,
    pub edits: Vec<ConfigEdit>,
    pub restart_required: bool,
    pub message: String,
}

/// Apply the plan after an explicit consent flag. The plan is recomputed from
/// the host, never taken from the webview.
pub fn apply_config(
    app_data: &Path,
    report: &StreamHostReport,
    requested_output: Option<&str>,
    requested_interface: Option<&str>,
    consent: bool,
) -> Result<ConfigApply, StreamHostError> {
    if !consent {
        return Err(StreamHostError::new(
            "consent_required",
            "Writing sunshine.conf needs explicit consent: review the dry-run plan first.",
        ));
    }
    let plan = plan_config(report, requested_output, requested_interface)?;
    if let Some(reason) = &plan.blocked {
        return Err(StreamHostError::new("blocked", reason.clone()));
    }
    if !plan.applicable {
        return Ok(ConfigApply {
            path: plan.path.clone(),
            backup: plan.backup.clone(),
            written: false,
            sha256: String::new(),
            edits: plan.edits.clone(),
            restart_required: false,
            message: plan.message.clone(),
        });
    }
    let path = PathBuf::from(&plan.path);
    let original = std::fs::read(&path).unwrap_or_default();
    let sha_before = sha256_hex(&original);
    let exists = path.is_file();
    let mut state = read_state(app_data);

    // Reuse the pristine backup across successive applies; overwriting it with
    // an already-edited file would destroy the byte-identical revert.
    let backup = PathBuf::from(&plan.backup);
    let prior = state
        .config
        .as_ref()
        .filter(|config| config.path == plan.path && config.existed == exists);
    let reuse = prior.is_some_and(|_| exists && backup.is_file())
        || prior.is_some_and(|config| !exists && !config.existed);
    let (recorded_sha, recorded_existed) = match prior.filter(|_| reuse) {
        Some(config) => (config.sha_before.clone(), config.existed),
        None => (sha_before.clone(), exists),
    };
    if !reuse && exists && std::fs::write(&backup, &original).is_err() {
        // `/etc/sunshine` may be root-owned even when the file is readable:
        // install the backup through polkit so it is still byte-identical.
        stage_and_install(&backup, &original, "0644")?;
    }
    let original_text = String::from_utf8_lossy(&original).to_string();
    let writes = vec![
        ("output_name".to_string(), plan.output.clone()),
        ("stream_audio".to_string(), "on".to_string()),
        ("packet_size".to_string(), plan.packet_size.to_string()),
    ];
    let (contents, edits) = merge_config(&original_text, &writes);
    let mode = plan
        .files
        .iter()
        .find(|file| file.action == "write")
        .map(|file| file.mode.clone())
        .unwrap_or_else(|| "0644".to_string());
    write_config_bytes(&path, contents.as_bytes(), &mode)?;
    let written = std::fs::read(&path).map_err(|error| {
        StreamHostError::new(
            "write_failed",
            format!("Could not read back {}: {error}", path.display()),
        )
    })?;
    let sha256 = sha256_hex(&written);
    if sha256 != sha256_hex(contents.as_bytes()) {
        return Err(StreamHostError::new(
            "write_failed",
            format!("{} did not keep the planned contents.", path.display()),
        ));
    }
    state.config = Some(ConfigState {
        path: plan.path.clone(),
        backup: plan.backup.clone(),
        existed: recorded_existed,
        sha_before: if recorded_existed {
            recorded_sha
        } else {
            String::new()
        },
        applied_at: crate::util::now_rfc3339(),
    });
    write_state(app_data, &state)?;
    Ok(ConfigApply {
        path: plan.path,
        backup: plan.backup,
        written: true,
        sha256,
        edits,
        restart_required: true,
        message: "sunshine.conf updated. Restart Sunshine so it captures the new output."
            .to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRevert {
    pub restored: bool,
    pub path: String,
    pub removed: bool,
    pub sha256: Option<String>,
    pub message: String,
}

/// Restore the backup byte for byte. Refuses when the backup does not match the
/// recorded hash, instead of overwriting the file with unknown bytes.
pub fn revert_config(app_data: &Path) -> Result<ConfigRevert, StreamHostError> {
    let mut state = read_state(app_data);
    let Some(config) = state.config.clone() else {
        return Err(StreamHostError::new(
            "nothing_to_revert",
            "Ludex has no recorded sunshine.conf change on this machine.",
        ));
    };
    let path = PathBuf::from(&config.path);
    let backup = PathBuf::from(&config.backup);
    if config.existed {
        let bytes = std::fs::read(&backup).map_err(|error| {
            StreamHostError::new(
                "backup_missing",
                format!("The backup {} is missing: {error}", backup.display()),
            )
        })?;
        if sha256_hex(&bytes) != config.sha_before {
            return Err(StreamHostError::new(
                "backup_mismatch",
                "The backup no longer matches the recorded checksum, so Ludex will not restore it.",
            ));
        }
        let mode = file_mode(&path);
        write_config_bytes(&path, &bytes, &mode)?;
        let restored = std::fs::read(&path).unwrap_or_default();
        let sha256 = sha256_hex(&restored);
        if sha256 != config.sha_before {
            return Err(StreamHostError::new(
                "revert_failed",
                format!("{} was not restored byte for byte.", path.display()),
            ));
        }
        let _ = remove_config_file(&backup);
        state.config = None;
        write_state(app_data, &state)?;
        Ok(ConfigRevert {
            restored: true,
            path: config.path,
            removed: false,
            sha256: Some(sha256),
            message: "sunshine.conf was restored from its backup.".to_string(),
        })
    } else {
        remove_config_file(&path)?;
        let _ = remove_config_file(&backup);
        state.config = None;
        write_state(app_data, &state)?;
        Ok(ConfigRevert {
            restored: true,
            path: config.path,
            removed: true,
            sha256: None,
            message: "The sunshine.conf Ludex created was removed.".to_string(),
        })
    }
}

/* -------------------------------- firewall -------------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallTool {
    Ufw,
    Firewalld,
    Nftables,
}

impl FirewallTool {
    pub fn id(self) -> &'static str {
        match self {
            FirewallTool::Ufw => "ufw",
            FirewallTool::Firewalld => "firewalld",
            FirewallTool::Nftables => "nftables",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallRule {
    pub protocol: String,
    pub first: u16,
    pub last: u16,
    pub purpose: String,
}

impl FirewallRule {
    pub fn id(&self) -> String {
        format!("{}:{}-{}", self.protocol, self.first, self.last)
    }
}

/// The exact Sunshine ranges, verified against upstream's documented ports.
/// TCP 47990 is the Web UI (never exposed to the WAN by remote play); it stays
/// open on the host because Sunshine serves it on the same listener.
pub fn sunshine_rules() -> Vec<FirewallRule> {
    let mut rules = Vec::new();
    for (first, last) in TCP_RANGES {
        rules.push(FirewallRule {
            protocol: "tcp".to_string(),
            first,
            last,
            purpose: if first == 48010 {
                "RTSP control".to_string()
            } else {
                "HTTPS, HTTP pairing and the Web UI".to_string()
            },
        });
    }
    for (first, last) in UDP_RANGES {
        rules.push(FirewallRule {
            protocol: "udp".to_string(),
            first,
            last,
            purpose: "video, control, audio, microphone and RTSP".to_string(),
        });
    }
    rules
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallEnv {
    pub ufw_binary: bool,
    pub ufw_active: Option<bool>,
    pub firewalld_binary: bool,
    pub firewalld_active: bool,
    pub nft_binary: bool,
    pub pkexec: bool,
}

impl FirewallEnv {
    pub fn detect() -> Self {
        let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        let has = |name: &str| {
            path_dirs
                .iter()
                .map(|dir| dir.join(name))
                .chain([
                    PathBuf::from("/usr/sbin").join(name),
                    PathBuf::from("/sbin").join(name),
                ])
                .any(|path| path.is_file())
        };
        let service_active = |unit: &str| {
            run_tool(&path_dirs, "systemctl", &["is-active", unit])
                .is_some_and(|output| output.success && output.text.trim() == "active")
        };
        Self {
            ufw_binary: has("ufw"),
            ufw_active: if has("ufw") {
                Some(service_active("ufw.service"))
            } else {
                None
            },
            firewalld_binary: has("firewall-cmd"),
            firewalld_active: has("firewall-cmd") && service_active("firewalld.service"),
            nft_binary: has("nft"),
            pkexec: has("pkexec"),
        }
    }
}

pub fn detect_firewall(env: &FirewallEnv) -> Option<FirewallTool> {
    if env.ufw_binary && env.ufw_active.unwrap_or(true) {
        return Some(FirewallTool::Ufw);
    }
    if env.firewalld_binary && env.firewalld_active {
        return Some(FirewallTool::Firewalld);
    }
    if env.nft_binary {
        return Some(FirewallTool::Nftables);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallPlan {
    pub tool: String,
    pub label: String,
    pub rules: Vec<FirewallRule>,
    /// Individual commands the elevated script runs.
    pub commands: Vec<String>,
    /// True when the rules were recorded as added by a previous Ludex run.
    pub applied: bool,
    pub needs_elevation: bool,
    pub supported: bool,
    pub blocked: Option<String>,
    pub message: String,
}

fn rule_spec(rule: &FirewallRule, separator: char) -> String {
    if rule.first == rule.last {
        return format!("{}/{}", rule.first, rule.protocol);
    }
    format!("{}{separator}{}/{}", rule.first, rule.last, rule.protocol)
}

fn ufw_rule(rule: &FirewallRule) -> String {
    if rule.first == rule.last {
        format!("{}/{}", rule.first, rule.protocol)
    } else {
        format!("{}:{}/{}", rule.first, rule.last, rule.protocol)
    }
}

/// Commands that open the missing rules with one tool.
pub fn firewall_open_commands(tool: FirewallTool, rules: &[FirewallRule]) -> Vec<String> {
    match tool {
        FirewallTool::Ufw => rules
            .iter()
            .map(|rule| format!("ufw allow {}", ufw_rule(rule)))
            .collect(),
        FirewallTool::Firewalld => {
            let mut commands: Vec<String> = rules
                .iter()
                .map(|rule| {
                    format!(
                        "firewall-cmd --permanent --add-port={}",
                        rule_spec(rule, '-')
                    )
                })
                .collect();
            commands.push("firewall-cmd --reload".to_string());
            commands
        }
        FirewallTool::Nftables => {
            let mut commands = vec![
                format!("nft add table inet {NFT_TABLE}"),
                format!(
                    "nft 'add chain inet {NFT_TABLE} input {{ type filter hook input priority filter; policy accept; }}'"
                ),
            ];
            for protocol in ["tcp", "udp"] {
                let ports: Vec<String> = rules
                    .iter()
                    .filter(|rule| rule.protocol == protocol)
                    .map(|rule| {
                        if rule.first == rule.last {
                            rule.first.to_string()
                        } else {
                            format!("{}-{}", rule.first, rule.last)
                        }
                    })
                    .collect();
                if !ports.is_empty() {
                    commands.push(format!(
                        "nft add rule inet {NFT_TABLE} input {protocol} dport {{ {} }} accept",
                        ports.join(", ")
                    ));
                }
            }
            commands
        }
    }
}

/// Commands that close only the rule ids Ludex recorded as added.
pub fn firewall_close_commands(
    tool: FirewallTool,
    added: &[String],
    created_table: bool,
) -> Vec<String> {
    let rules = sunshine_rules();
    match tool {
        FirewallTool::Ufw => added
            .iter()
            .filter_map(|id| rules.iter().find(|rule| &rule.id() == id))
            .map(|rule| format!("ufw delete allow {}", ufw_rule(rule)))
            .collect(),
        FirewallTool::Firewalld => {
            let mut commands: Vec<String> = added
                .iter()
                .filter_map(|id| rules.iter().find(|rule| &rule.id() == id))
                .map(|rule| {
                    format!(
                        "firewall-cmd --permanent --remove-port={}",
                        rule_spec(rule, '-')
                    )
                })
                .collect();
            if !commands.is_empty() {
                commands.push("firewall-cmd --reload".to_string());
            }
            commands
        }
        FirewallTool::Nftables => {
            if created_table {
                vec![format!("nft delete table inet {NFT_TABLE}")]
            } else {
                Vec::new()
            }
        }
    }
}

/// Read the rules that already exist, so a revert never removes a rule Ludex
/// did not add. Runs as root (ufw/firewalld/nft all require it).
pub fn firewall_probe_script(tool: FirewallTool) -> String {
    match tool {
        FirewallTool::Ufw => "ufw status".to_string(),
        FirewallTool::Firewalld => {
            let mut lines = Vec::new();
            for rule in sunshine_rules() {
                lines.push(format!(
                    "firewall-cmd --permanent --query-port={} && echo present={} || true",
                    rule_spec(&rule, '-'),
                    rule.id()
                ));
            }
            lines.join("\n")
        }
        FirewallTool::Nftables => format!("nft list table inet {NFT_TABLE} 2>/dev/null || true"),
    }
}

/// Rule ids present in the probe output.
pub fn parse_firewall_presence(tool: FirewallTool, output: &str) -> BTreeSet<String> {
    let mut present = BTreeSet::new();
    match tool {
        FirewallTool::Ufw => {
            for line in output.lines() {
                let token = line.split_whitespace().next().unwrap_or_default();
                for rule in sunshine_rules() {
                    if token == ufw_rule(&rule) {
                        present.insert(rule.id());
                    }
                }
            }
        }
        FirewallTool::Firewalld => {
            for line in output.lines() {
                if let Some(id) = line.trim().strip_prefix("present=") {
                    present.insert(id.trim().to_string());
                }
            }
        }
        FirewallTool::Nftables => {
            // Only the rules actually present in Ludex's own table. The table
            // must exist for this output to contain anything.
            for line in output.lines() {
                let line = line.trim();
                if line.starts_with("tcp dport") {
                    for rule in sunshine_rules()
                        .iter()
                        .filter(|rule| rule.protocol == "tcp")
                    {
                        if line.contains(&format!("{}", rule.first))
                            && line.contains(&format!("{}", rule.last))
                        {
                            present.insert(rule.id());
                        }
                    }
                }
                if line.starts_with("udp dport") {
                    for rule in sunshine_rules()
                        .iter()
                        .filter(|rule| rule.protocol == "udp")
                    {
                        if line.contains(&format!("{}", rule.first))
                            && line.contains(&format!("{}", rule.last))
                        {
                            present.insert(rule.id());
                        }
                    }
                }
            }
        }
    }
    present
}

/// True when the probe output shows Ludex's nftables table already exists.
pub fn firewall_table_exists(tool: FirewallTool, output: &str) -> bool {
    tool == FirewallTool::Nftables && output.contains(&format!("table inet {NFT_TABLE}"))
}

/// Build the firewall plan shown to the user before any prompt.
pub fn plan_firewall(env: &FirewallEnv, state: &StreamHostState) -> FirewallPlan {
    let Some(tool) = detect_firewall(env) else {
        return FirewallPlan {
            tool: "none".to_string(),
            label: "No supported firewall".to_string(),
            rules: sunshine_rules(),
            commands: Vec::new(),
            applied: state.firewall.is_some(),
            needs_elevation: false,
            supported: false,
            blocked: Some(
                "No ufw, firewalld or nftables was found. Configure your firewall manually with the ports below."
                    .to_string(),
            ),
            message: "Ports were not opened.".to_string(),
        };
    };
    let rules = sunshine_rules();
    FirewallPlan {
        tool: tool.id().to_string(),
        label: match tool {
            FirewallTool::Ufw => "ufw".to_string(),
            FirewallTool::Firewalld => "firewalld".to_string(),
            FirewallTool::Nftables => "nftables".to_string(),
        },
        commands: firewall_open_commands(tool, &rules),
        rules,
        applied: state
            .firewall
            .as_ref()
            .is_some_and(|firewall| firewall.tool == tool.id()),
        needs_elevation: env.pkexec,
        supported: true,
        blocked: if env.pkexec {
            None
        } else {
            Some("pkexec is missing; install polkit to open the ports from Ludex.".to_string())
        },
        message: format!(
            "{} rule(s) will be opened after the polkit prompt. No other port is touched.",
            TCP_RANGES.len() + UDP_RANGES.len()
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirewallReport {
    pub tool: String,
    pub opened: Vec<String>,
    pub skipped: Vec<String>,
    pub commands: Vec<String>,
    pub message: String,
}

impl FirewallReport {
    fn new(tool: &str, message: impl Into<String>) -> Self {
        Self {
            tool: tool.to_string(),
            opened: Vec::new(),
            skipped: Vec::new(),
            commands: Vec::new(),
            message: message.into(),
        }
    }
}

pub fn apply_firewall(
    app_data: &Path,
    env: &FirewallEnv,
    consent: bool,
) -> Result<FirewallReport, StreamHostError> {
    if !consent {
        return Err(StreamHostError::new(
            "consent_required",
            "Opening firewall ports needs explicit consent: review the exact rules first.",
        ));
    }
    let Some(tool) = detect_firewall(env) else {
        return Err(StreamHostError::new(
            "firewall_unsupported",
            "No ufw, firewalld or nftables was found on this host.",
        ));
    };
    if !env.pkexec {
        return Err(StreamHostError::new(
            "elevation_unavailable",
            "pkexec is missing; install polkit to open the ports from Ludex.",
        ));
    }
    let probe = run_elevated(&firewall_probe_script(tool))?;
    let present = parse_firewall_presence(tool, &probe);
    let table_exists = firewall_table_exists(tool, &probe);
    let rules = sunshine_rules();
    let missing: Vec<FirewallRule> = rules
        .iter()
        .filter(|rule| !present.contains(&rule.id()))
        .cloned()
        .collect();
    let mut state = read_state(app_data);
    if missing.is_empty() {
        return Ok(FirewallReport {
            tool: tool.id().to_string(),
            opened: Vec::new(),
            skipped: rules.iter().map(FirewallRule::id).collect(),
            commands: Vec::new(),
            message: "Every Sunshine rule was already open; nothing was changed.".to_string(),
        });
    }
    let commands = if tool == FirewallTool::Nftables && table_exists {
        // The table is ours already: empty it instead of failing on `add table`,
        // then write the documented rules back.
        let mut commands = vec![format!("nft flush table inet {NFT_TABLE}")];
        commands.extend(
            firewall_open_commands(tool, &rules)
                .into_iter()
                .filter(|line| line.contains(" add rule ")),
        );
        commands
    } else {
        firewall_open_commands(tool, &missing)
    };
    let script = format!("set -eu\n{}\n", commands.join("\n"));
    if let Err(error) = run_elevated(&script) {
        // Some ports may be open while the state says nothing: close what this
        // attempt targeted, best effort, then report the original failure.
        let targets: Vec<String> = missing.iter().map(FirewallRule::id).collect();
        let rollback = firewall_close_commands(tool, &targets, !table_exists);
        if !rollback.is_empty() {
            let script = format!(
                "set -u\n{}\n",
                rollback
                    .iter()
                    .map(|command| format!("{command} 2>/dev/null || true"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            let _ = run_elevated(&script);
        }
        return Err(error);
    }
    let created_table = tool == FirewallTool::Nftables && !table_exists;
    let applied: Vec<String> = if tool == FirewallTool::Nftables && table_exists {
        rules.iter().map(FirewallRule::id).collect()
    } else {
        missing.iter().map(FirewallRule::id).collect()
    };
    let mut firewall = state.firewall.clone().unwrap_or_default();
    if firewall.tool != tool.id() {
        firewall = FirewallState {
            tool: tool.id().to_string(),
            ..Default::default()
        };
    }
    for id in &applied {
        if !firewall.added.contains(id) {
            firewall.added.push(id.clone());
        }
    }
    firewall.created_table |= created_table;
    firewall.applied_at = crate::util::now_rfc3339();
    state.firewall = Some(firewall);
    write_state(app_data, &state)?;
    Ok(FirewallReport {
        tool: tool.id().to_string(),
        opened: missing.iter().map(FirewallRule::id).collect(),
        skipped: rules
            .iter()
            .filter(|rule| present.contains(&rule.id()))
            .map(FirewallRule::id)
            .collect(),
        commands,
        message: format!("{} Sunshine rule(s) opened.", missing.len()),
    })
}

pub fn revert_firewall(
    app_data: &Path,
    env: &FirewallEnv,
) -> Result<FirewallReport, StreamHostError> {
    let Some(tool) = detect_firewall(env) else {
        return Err(StreamHostError::new(
            "firewall_unsupported",
            "No supported firewall is running; no Ludex rule can be closed.",
        ));
    };
    let mut state = read_state(app_data);
    let Some(firewall) = state.firewall.clone() else {
        return Err(StreamHostError::new(
            "nothing_to_revert",
            "Ludex did not open any firewall rule on this machine.",
        ));
    };
    if firewall.tool != tool.id() {
        return Err(StreamHostError::new(
            "firewall_changed",
            format!(
                "The firewall changed since Ludex opened the ports ({} is now active instead of {}). Close the rules with your firewall tool.",
                tool.id(),
                firewall.tool
            ),
        ));
    }
    let commands = firewall_close_commands(tool, &firewall.added, firewall.created_table);
    if commands.is_empty() {
        state.firewall = None;
        write_state(app_data, &state)?;
        return Ok(FirewallReport::new(
            tool.id(),
            "No rule recorded by Ludex is still open.",
        ));
    }
    let script = format!(
        "set -u\n{}\n",
        commands
            .iter()
            .map(|command| format!("{command} 2>/dev/null || true"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    run_elevated(&script)?;
    state.firewall = None;
    write_state(app_data, &state)?;
    Ok(FirewallReport {
        tool: tool.id().to_string(),
        opened: Vec::new(),
        skipped: Vec::new(),
        commands,
        message: format!("{} Ludex rule(s) closed.", firewall.added.len()),
    })
}

/* -------------------------------- pairing -------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Anything that can perform one local HTTP exchange. Tests inject a fake so no
/// socket is ever opened.
pub trait PairHttp {
    fn request(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
}

pub struct TcpHttp;

impl PairHttp for TcpHttp {
    fn request(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let timeout = Duration::from_secs(4);
        let addresses = (request.host.as_str(), request.port)
            .to_socket_addrs()
            .map_err(|error| format!("Could not resolve {}: {error}", request.host))?;
        let mut last_error = String::new();
        for address in addresses {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(mut stream) => {
                    let _ = stream.set_read_timeout(Some(timeout));
                    let _ = stream.set_write_timeout(Some(timeout));
                    let mut head = format!(
                        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
                        request.method, request.path, request.host, request.port
                    );
                    for (name, value) in &request.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str(&format!("Content-Length: {}\r\n\r\n", request.body.len()));
                    head.push_str(&request.body);
                    stream
                        .write_all(head.as_bytes())
                        .map_err(|error| format!("Could not send the request: {error}"))?;
                    let mut raw = String::new();
                    let mut buffer = [0u8; 4096];
                    loop {
                        match stream.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(count) => {
                                raw.push_str(&String::from_utf8_lossy(&buffer[..count]));
                            }
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                // Sunshine may keep the connection open; the
                                // response read so far is what matters.
                                break;
                            }
                            Err(error) => {
                                return Err(format!("Could not read the response: {error}"));
                            }
                        }
                    }
                    return parse_http_response(&raw);
                }
                Err(error) => last_error = format!("{address}: {error}"),
            }
        }
        Err(format!("Could not connect to Sunshine: {last_error}"))
    }
}

pub fn parse_http_response(raw: &str) -> Result<HttpResponse, String> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .or_else(|| raw.split_once("\n\n"))
        .ok_or_else(|| "Sunshine returned an incomplete HTTP response.".to_string())?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| format!("Sunshine returned an unreadable status line: {status_line}"))?;
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        decode_chunked(body)
    } else {
        body.to_string()
    };
    Ok(HttpResponse { status, body })
}

fn decode_chunked(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size_line, tail)) = rest.split_once("\r\n").or_else(|| rest.split_once('\n')) {
        let Ok(size) = usize::from_str_radix(size_line.trim(), 16) else {
            break;
        };
        if size == 0 {
            break;
        }
        let take = tail.len().min(size);
        out.push_str(&tail[..take]);
        rest = tail[take..].trim_start_matches(['\r', '\n']);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingOutcome {
    /// `paired`, `pin-rejected`, `credentials-required`, `unreachable`,
    /// `invalid-pin`, `browser` or `failed`.
    pub mode: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<usize>,
    pub credentials_saved: bool,
    pub message: String,
}

fn auth_header(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64_encode(format!("{username}:{password}").as_bytes())
    )
}

/// `POST /api/pin`, the endpoint upstream Sunshine serves in
/// `src/confighttp.cpp` (`savePin`). The body is `{"pin":"1234","name":"Ludex"}`.
pub fn pin_request(
    username: &str,
    password: &str,
    pin: &str,
    name: &str,
    api_port: u16,
) -> HttpRequest {
    HttpRequest {
        method: "POST".to_string(),
        host: "127.0.0.1".to_string(),
        port: api_port,
        path: "/api/pin".to_string(),
        headers: vec![
            ("Authorization".to_string(), auth_header(username, password)),
            ("Content-Type".to_string(), "application/json".to_string()),
        ],
        body: serde_json::json!({ "pin": pin, "name": name }).to_string(),
    }
}

/// `GET /api/apps`, used to confirm the listener is alive after pairing.
pub fn apps_request(username: &str, password: &str, api_port: u16) -> HttpRequest {
    HttpRequest {
        method: "GET".to_string(),
        host: "127.0.0.1".to_string(),
        port: api_port,
        path: "/api/apps".to_string(),
        headers: vec![("Authorization".to_string(), auth_header(username, password))],
        body: String::new(),
    }
}

fn count_apps(body: &str) -> Option<usize> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("apps")
        .and_then(|apps| apps.as_array())
        .map(Vec::len)
}

/// Pair through the local API. Returns a typed outcome instead of an error so
/// the UI can always offer the browser fallback.
pub fn pair_with(
    http: &dyn PairHttp,
    username: Option<&str>,
    password: Option<&str>,
    pin: &str,
    name: &str,
    api_port: u16,
) -> PairingOutcome {
    let url = format!("http://127.0.0.1:{api_port}/");
    let valid_pin = pin.len() == PIN_LEN && pin.chars().all(|character| character.is_ascii_digit());
    if !valid_pin {
        return PairingOutcome {
            mode: "invalid-pin".to_string(),
            url,
            status: None,
            apps: None,
            credentials_saved: false,
            message: format!("Moonlight shows a {PIN_LEN}-digit PIN; type exactly those digits."),
        };
    }
    let (Some(username), Some(password)) = (
        username.map(str::trim).filter(|value| !value.is_empty()),
        password.filter(|value| !value.is_empty()),
    ) else {
        return PairingOutcome {
            mode: "credentials-required".to_string(),
            url,
            status: None,
            apps: None,
            credentials_saved: false,
            message:
                "Sunshine's admin username and password are needed for the API. Enter them here, or use the Sunshine page that just opened."
                    .to_string(),
        };
    };
    match http.request(&pin_request(username, password, pin, name, api_port)) {
        Ok(response) if response.status == 200 => {
            let accepted = response
                .body
                .to_ascii_lowercase()
                .contains("\"status\":\"true\"")
                || response
                    .body
                    .to_ascii_lowercase()
                    .contains("\"status\": \"true\"");
            if !accepted
                && response
                    .body
                    .to_ascii_lowercase()
                    .contains("\"status\":\"false\"")
            {
                return PairingOutcome {
                    mode: "pin-rejected".to_string(),
                    url,
                    status: Some(response.status),
                    apps: None,
                    credentials_saved: false,
                    message:
                        "Sunshine rejected the PIN. Check the number Moonlight shows and try again."
                            .to_string(),
                };
            }
            let apps = http
                .request(&apps_request(username, password, api_port))
                .ok()
                .filter(|response| response.status == 200)
                .and_then(|response| count_apps(&response.body));
            PairingOutcome {
                mode: "paired".to_string(),
                url,
                status: Some(response.status),
                apps,
                credentials_saved: false,
                message: match apps {
                    Some(count) => format!("Paired. Sunshine lists {count} app(s)."),
                    None => "Paired. Sunshine accepted the PIN.".to_string(),
                },
            }
        }
        Ok(response) if response.status == 401 || response.status == 403 => PairingOutcome {
            mode: "credentials-required".to_string(),
            url,
            status: Some(response.status),
            apps: None,
            credentials_saved: false,
            message: "Sunshine refused the admin credentials. Check the username and password."
                .to_string(),
        },
        Ok(response) => PairingOutcome {
            mode: "failed".to_string(),
            url,
            status: Some(response.status),
            apps: None,
            credentials_saved: false,
            message: format!(
                "Sunshine answered HTTP {}; open its page on this host to pair.",
                response.status
            ),
        },
        Err(error) => PairingOutcome {
            mode: "unreachable".to_string(),
            url,
            status: None,
            apps: None,
            credentials_saved: false,
            message: format!(
                "Sunshine's local API did not answer ({error}). Open its page on this host instead."
            ),
        },
    }
}

/* -------------------------------- commands -------------------------------- */

fn local_conf_path(settings: &LocalSettings) -> Option<PathBuf> {
    let override_path = settings.sunshine_conf_path.trim();
    if !override_path.is_empty() {
        return Some(PathBuf::from(override_path));
    }
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(home.join(".config/sunshine/sunshine.conf"));
    }
    candidates.push(PathBuf::from("/etc/sunshine/sunshine.conf"));
    candidates.into_iter().find(|path| path.is_file())
}

fn local_api_port(settings: &LocalSettings) -> u16 {
    let text = local_conf_path(settings)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    sunshine_port_base(&parse_sunshine_conf(&text))
}

fn report_for(settings: &LocalSettings) -> StreamHostReport {
    detect(settings)
}

#[tauri::command]
pub async fn stream_host_status(
    app: tauri::AppHandle,
) -> Result<StreamHostReport, StreamHostError> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let app_data = state.app_data.clone();
    let stored = state.secrets.get(SECRET_USER).ok().flatten().is_some();
    tauri::async_runtime::spawn_blocking(move || {
        let mut report = report_for(&settings);
        report.credentials_stored = stored;
        let recorded = read_state(&app_data);
        report.config_revertible = recorded.config.is_some();
        report.firewall_revertible = recorded.firewall.is_some();
        Ok::<StreamHostReport, StreamHostError>(report)
    })
    .await
    .map_err(|error| {
        StreamHostError::new(
            "probe_failed",
            format!("The streaming host probe stopped: {error}"),
        )
    })?
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfTestReport {
    pub host: StreamHostReport,
    pub self_test: SelfTest,
}

#[tauri::command]
pub async fn stream_host_self_test(
    app: tauri::AppHandle,
) -> Result<SelfTestReport, StreamHostError> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let app_data = state.app_data.clone();
    let stored = state.secrets.get(SECRET_USER).ok().flatten().is_some();
    tauri::async_runtime::spawn_blocking(move || {
        let mut host = report_for(&settings);
        host.credentials_stored = stored;
        let recorded = read_state(&app_data);
        host.config_revertible = recorded.config.is_some();
        host.firewall_revertible = recorded.firewall.is_some();
        let self_test = self_test(&host);
        Ok::<SelfTestReport, StreamHostError>(SelfTestReport { host, self_test })
    })
    .await
    .map_err(|error| {
        StreamHostError::new("probe_failed", format!("The self-test stopped: {error}"))
    })?
}

#[tauri::command]
pub async fn stream_host_config_plan(
    app: tauri::AppHandle,
    output: Option<String>,
    interface: Option<String>,
) -> Result<StreamConfigPlan, StreamHostError> {
    use tauri::Manager;
    let settings = app.state::<AppState>().settings_snapshot();
    tauri::async_runtime::spawn_blocking(move || {
        let report = report_for(&settings);
        plan_config(&report, output.as_deref(), interface.as_deref())
    })
    .await
    .map_err(|error| StreamHostError::new("plan_failed", format!("The plan stopped: {error}")))?
}

#[tauri::command]
pub async fn stream_host_config_apply(
    app: tauri::AppHandle,
    output: Option<String>,
    interface: Option<String>,
    consent: bool,
) -> Result<ConfigApply, StreamHostError> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let app_data = state.app_data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = report_for(&settings);
        apply_config(
            &app_data,
            &report,
            output.as_deref(),
            interface.as_deref(),
            consent,
        )
    })
    .await
    .map_err(|error| StreamHostError::new("apply_failed", format!("The write stopped: {error}")))?
}

#[tauri::command]
pub async fn stream_host_config_revert(
    app: tauri::AppHandle,
) -> Result<ConfigRevert, StreamHostError> {
    use tauri::Manager;
    let app_data = app.state::<AppState>().app_data.clone();
    tauri::async_runtime::spawn_blocking(move || revert_config(&app_data))
        .await
        .map_err(|error| {
            StreamHostError::new("revert_failed", format!("The revert stopped: {error}"))
        })?
}

#[tauri::command]
pub async fn stream_host_firewall_plan(
    app: tauri::AppHandle,
) -> Result<FirewallPlan, StreamHostError> {
    use tauri::Manager;
    let app_data = app.state::<AppState>().app_data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(plan_firewall(
            &FirewallEnv::detect(),
            &read_state(&app_data),
        ))
    })
    .await
    .map_err(|error| {
        StreamHostError::new("plan_failed", format!("The firewall plan stopped: {error}"))
    })?
}

#[tauri::command]
pub async fn stream_host_firewall_apply(
    app: tauri::AppHandle,
    consent: bool,
) -> Result<FirewallReport, StreamHostError> {
    use tauri::Manager;
    let app_data = app.state::<AppState>().app_data.clone();
    tauri::async_runtime::spawn_blocking(move || {
        apply_firewall(&app_data, &FirewallEnv::detect(), consent)
    })
    .await
    .map_err(|error| {
        StreamHostError::new(
            "apply_failed",
            format!("The firewall change stopped: {error}"),
        )
    })?
}

#[tauri::command]
pub async fn stream_host_firewall_revert(
    app: tauri::AppHandle,
) -> Result<FirewallReport, StreamHostError> {
    use tauri::Manager;
    let app_data = app.state::<AppState>().app_data.clone();
    tauri::async_runtime::spawn_blocking(move || revert_firewall(&app_data, &FirewallEnv::detect()))
        .await
        .map_err(|error| {
            StreamHostError::new(
                "revert_failed",
                format!("The firewall revert stopped: {error}"),
            )
        })?
}

#[tauri::command]
pub async fn stream_host_pair(
    app: tauri::AppHandle,
    pin: String,
    username: Option<String>,
    password: Option<String>,
    remember: bool,
) -> Result<PairingOutcome, StreamHostError> {
    use tauri::Manager;
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let mut username = username.map(|value| value.trim().to_string());
    if username.as_deref().unwrap_or_default().is_empty() {
        username = state.secrets.get(SECRET_USER).ok().flatten();
    }
    let mut password = password.filter(|value| !value.is_empty());
    let saved_password = state.secrets.get(SECRET_PASSWORD).ok().flatten();
    if password.is_none() {
        password = saved_password;
    }
    let settings_for_ports = settings.clone();
    let secret_user = username.clone();
    let secret_password = password.clone();
    let mut outcome = tauri::async_runtime::spawn_blocking(move || {
        pair_with(
            &TcpHttp,
            username.as_deref(),
            password.as_deref(),
            &pin,
            PAIR_NAME,
            local_api_port(&settings_for_ports),
        )
    })
    .await
    .map_err(|error| {
        StreamHostError::new(
            "pair_failed",
            format!("The pairing request stopped: {error}"),
        )
    })?;
    if remember && outcome.mode == "paired" {
        if let (Some(username), Some(password)) = (secret_user, secret_password) {
            let saved = state
                .secrets
                .set(SECRET_USER, &username)
                .and_then(|_| state.secrets.set(SECRET_PASSWORD, &password));
            outcome.credentials_saved = saved.is_ok();
        }
    }
    Ok(outcome)
}

/// Open Sunshine's own page in a Ludex window, for the case where the API has no
/// credentials yet. Falls back to the system browser and says why.
#[tauri::command]
pub fn stream_host_pairing_page(app: tauri::AppHandle) -> Result<PairingOutcome, StreamHostError> {
    use tauri::Manager;
    use tauri_plugin_opener::OpenerExt;
    const LABEL: &str = "sunshine-pairing";
    let settings = app.state::<AppState>().settings_snapshot();
    let base = local_api_port(&settings);
    let (_, _, web) = sunshine_web_ports(base);
    let url = format!("http://127.0.0.1:{web}/");
    let shown = url.clone();
    let builder = tauri::WebviewWindowBuilder::new(
        &app,
        LABEL,
        tauri::WebviewUrl::External(url.parse().map_err(|error| {
            StreamHostError::new("bad_url", format!("Could not build {url}: {error}"))
        })?),
    )
    .title("Sunshine pairing")
    .inner_size(980.0, 760.0);
    match builder.build() {
        Ok(window) => {
            let _ = window.set_focus();
            Ok(PairingOutcome {
                mode: "browser".to_string(),
                url,
                status: None,
                apps: None,
                credentials_saved: false,
                message: "Enter the PIN Moonlight shows in this window. Ludex uses the API when admin credentials are available."
                    .to_string(),
            })
        }
        Err(error) => {
            let opened = app.opener().open_url(url.clone(), None::<&str>);
            Ok(PairingOutcome {
                mode: "browser".to_string(),
                url,
                status: None,
                apps: None,
                credentials_saved: false,
                message: match opened {
                    Ok(()) => format!(
                        "Ludex could not open a window ({error}), so the Sunshine page was opened in your browser."
                    ),
                    Err(open_error) => format!(
                        "Ludex could not open a window ({error}) or a browser ({open_error}). Visit {shown} on the host."
                    ),
                },
            })
        }
    }
}

/* --------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-stream-host-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    fn output(root: &Path, connector: &str, connected: bool, dpms: &str) -> PathBuf {
        let dir = root.join("drm").join(format!("card0-{connector}"));
        write(
            &dir.join("status"),
            if connected {
                "connected"
            } else {
                "disconnected"
            },
        );
        write(&dir.join("dpms"), dpms);
        write(&dir.join("modes"), "1920x1080\n1280x720\n");
        dir
    }

    fn interface(root: &Path, name: &str, mtu: u16, state: &str) {
        let dir = root.join("net").join(name);
        write(&dir.join("mtu"), &mtu.to_string());
        write(&dir.join("operstate"), state);
    }

    /// A host where every probe succeeds; individual tests break one thing.
    fn ready_env(root: &Path) -> StreamHostEnv {
        let bin = root.join("bin/sunshine");
        write(&bin, "#!/bin/sh\nexit 0\n");
        let unit = root.join("units/sunshine.service");
        write(&unit, "[Unit]\nDescription=Sunshine\n");
        let conf = root.join("sunshine.conf");
        write(&conf, "# Sunshine configuration\nfps = 60\n");
        output(root, "DP-1", true, "On");
        interface(root, "eth0", 1500, "up");
        interface(root, "wg0", 1280, "up");
        write(
            &root.join("route"),
            "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t0101A8C0\t0003\n",
        );
        write(&root.join("dri/card0"), "");
        write(&root.join("dri/renderD128"), "");
        StreamHostEnv {
            supported: true,
            path_dirs: vec![root.join("bin")],
            config_candidates: vec![conf],
            drm_dir: root.join("drm"),
            dev_dri: root.join("dri"),
            net_dir: root.join("net"),
            route_file: root.join("route"),
            system_unit_dirs: vec![root.join("units")],
            user_unit_dirs: Vec::new(),
            systemctl_enabled: Some(ToolOutput::new(true, "enabled")),
            systemctl_active: Some(ToolOutput::new(true, "active")),
            systemctl_user_active: None,
            loginctl: Some(ToolOutput::new(
                true,
                "Id=2\nType=wayland\nActive=yes\nSeat=seat0",
            )),
            version: Some(ToolOutput::new(true, "Sunshine version: v0.23.1")),
            help: None,
            vainfo: Some(ToolOutput::new(
                true,
                "VAProfileH264High : VAEntrypointEncSlice\nVAProfileHEVCMain : VAEntrypointEncSlice",
            )),
            ffmpeg: Some(ToolOutput::new(
                true,
                " V....D h264_vaapi VAAPI H.264\n V....D hevc_vaapi VAAPI HEVC",
            )),
        }
    }

    fn detached_env(root: &Path) -> StreamHostEnv {
        StreamHostEnv {
            supported: true,
            path_dirs: vec![root.join("empty-bin")],
            config_candidates: vec![root.join("missing/sunshine.conf")],
            drm_dir: root.join("no-drm"),
            dev_dri: root.join("no-dri"),
            net_dir: root.join("no-net"),
            route_file: root.join("no-route"),
            system_unit_dirs: vec![root.join("no-units")],
            user_unit_dirs: Vec::new(),
            systemctl_enabled: None,
            systemctl_active: None,
            systemctl_user_active: None,
            loginctl: None,
            version: None,
            help: None,
            vainfo: None,
            ffmpeg: None,
        }
    }

    fn codes(report: &SelfTest) -> Vec<String> {
        report
            .findings
            .iter()
            .map(|finding| finding.id.clone())
            .collect()
    }

    #[test]
    fn packet_size_follows_the_interface_mtu() {
        let table = [
            (0, 1024),
            (576, 1024),
            (1100, 1024),
            (1200, 1120),
            (1220, 1140),
            (1280, 1200),
            (1300, 1220),
            (1400, 1320),
            (1420, 1340),
            (1500, 1420),
            (9000, 1420),
        ];
        for (mtu, expected) in table {
            assert_eq!(packet_size_for_mtu(mtu), expected, "MTU {mtu}");
        }
    }

    #[test]
    fn the_tunnel_mtu_wins_over_the_default_route() {
        let root = temp_dir("mtu");
        interface(&root, "eth0", 1500, "up");
        interface(&root, "wg0", 1280, "up");
        let interfaces = parse_interfaces(&root.join("net"));
        let choice = choose_mtu(&interfaces, Some("eth0"), None).unwrap();
        assert_eq!(choice.interface, "wg0");
        assert_eq!(choice.mtu, 1280);
        assert_eq!(choice.packet_size, 1200);
        assert_eq!(choice.source, "tunnel");

        let choice = choose_mtu(&interfaces, Some("eth0"), Some("eth0")).unwrap();
        assert_eq!(choice.interface, "eth0");
        assert_eq!(choice.packet_size, 1420);
        assert_eq!(choice.source, "override");
    }

    #[test]
    fn the_default_route_is_parsed_and_used_without_a_tunnel() {
        let root = temp_dir("route");
        interface(&root, "eth0", 1500, "up");
        let interfaces = parse_interfaces(&root.join("net"));
        let route = parse_default_route(
            "Iface\tDestination\tGateway\tFlags\neth0\t00000000\t0101A8C0\t0003\n",
        );
        assert_eq!(route.as_deref(), Some("eth0"));
        let choice = choose_mtu(&interfaces, route.as_deref(), None).unwrap();
        assert_eq!(choice.source, "route");
        assert_eq!(choice.packet_size, 1420);
        assert_eq!(parse_default_route(""), None);
    }

    #[test]
    fn the_connected_output_is_chosen_and_a_wrong_pin_is_an_error() {
        let outputs = vec![
            DisplayOutput {
                name: "DP-1".into(),
                connected: true,
                active: Some(true),
                width: Some(1920),
                height: Some(1080),
            },
            DisplayOutput {
                name: "HDMI-A-1".into(),
                connected: false,
                active: None,
                width: None,
                height: None,
            },
        ];
        assert_eq!(choose_output(&outputs, None, None).unwrap(), "DP-1");
        assert_eq!(
            choose_output(&outputs, Some("HDMI-A-1"), None).unwrap(),
            "DP-1"
        );
        let error = choose_output(&outputs, Some("HDMI-A-1"), Some("HDMI-A-1")).unwrap_err();
        assert!(error.contains("HDMI-A-1 is not connected"));
    }

    #[test]
    fn the_config_merge_preserves_every_other_byte() {
        let original = "# keep me\r\nfps = 60\r\noutput_name = HDMI-A-1\r\n# trailing\r\n";
        let (merged, edits) = merge_config(
            original,
            &[
                ("output_name".to_string(), "DP-1".to_string()),
                ("stream_audio".to_string(), "on".to_string()),
                ("packet_size".to_string(), "1200".to_string()),
            ],
        );
        assert!(merged.starts_with("# keep me\r\nfps = 60\r\n"));
        assert!(merged.contains("output_name = DP-1\r\n"));
        assert!(!merged.contains("HDMI-A-1"));
        assert!(merged.contains("# trailing\r\n"));
        assert!(merged.contains("stream_audio = on\r\n"));
        assert!(merged.contains("packet_size = 1200\r\n"));
        assert_eq!(edits.len(), 3);
        assert_eq!(edits[0].previous.as_deref(), Some("HDMI-A-1"));
        assert_eq!(edits[1].previous, None);
    }

    #[test]
    fn the_last_duplicate_key_is_the_one_sunshine_keeps() {
        let original = "output_name = HDMI-A-1\noutput_name = DP-2\n";
        let (merged, edits) = merge_config(original, &[("output_name".into(), "DP-1".into())]);
        assert_eq!(merged, "output_name = HDMI-A-1\noutput_name = DP-1\n");
        assert_eq!(edits[0].previous.as_deref(), Some("DP-2"));
    }

    #[test]
    fn the_config_view_reads_the_three_keys() {
        let (output, audio, packet) =
            config_view("output_name = DP-1\nstream_audio = off\npacket_size = 1200\n");
        assert_eq!(output.as_deref(), Some("DP-1"));
        assert_eq!(audio.as_deref(), Some("off"));
        assert_eq!(packet, Some(1200));
    }

    #[test]
    fn detection_reports_missing_sunshine_distinctly() {
        let root = temp_dir("missing");
        let report = detect_with(&detached_env(&root));
        assert!(!report.detected);
        assert!(!report.ready);
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "sunshine")
                .unwrap()
                .status,
            CheckStatus::Missing
        );
        assert_eq!(codes(&self_test(&report)), vec!["sunshine-missing"]);
    }

    #[test]
    fn detection_reports_a_disabled_service() {
        let root = temp_dir("disabled");
        let mut env = ready_env(&root);
        env.systemctl_enabled = Some(ToolOutput::new(true, "disabled"));
        env.systemctl_active = Some(ToolOutput::new(true, "inactive"));
        let report = detect_with(&env);
        assert!(report.detected);
        assert!(!report.ready);
        assert!(codes(&self_test(&report)).contains(&"sunshine-disabled".to_string()));
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "service")
                .unwrap()
                .status,
            CheckStatus::Broken
        );
    }

    #[test]
    fn detection_reports_the_wrong_output() {
        let root = temp_dir("wrong-output");
        let env = ready_env(&root);
        write(&root.join("sunshine.conf"), "output_name = DP-9\n");
        let report = detect_with(&env);
        assert_eq!(report.configured_output.as_deref(), Some("DP-9"));
        assert_eq!(
            report
                .checks
                .iter()
                .find(|check| check.id == "output")
                .unwrap()
                .status,
            CheckStatus::Broken
        );
        assert!(codes(&self_test(&report)).contains(&"wrong-output".to_string()));
    }

    #[test]
    fn detection_reports_a_blanked_display_and_a_missing_encoder() {
        let root = temp_dir("blanked");
        let mut env = ready_env(&root);
        output(&root, "DP-1", true, "Off");
        env.vainfo = Some(ToolOutput::new(false, "vainfo: not found"));
        env.ffmpeg = Some(ToolOutput::new(false, "ffmpeg: not found"));
        let report = detect_with(&env);
        let found = codes(&self_test(&report));
        assert!(found.contains(&"display-blanked".to_string()));
        assert!(found.contains(&"encoder-unavailable".to_string()));
        assert!(!report.encoder.available);
    }

    #[test]
    fn detection_reports_no_connected_output() {
        let root = temp_dir("no-output");
        let env = ready_env(&root);
        output(&root, "DP-1", false, "Off");
        let report = detect_with(&env);
        let found = codes(&self_test(&report));
        assert!(found.contains(&"no-output".to_string()));
        assert!(!found.contains(&"capture-unavailable".to_string()));
    }

    #[test]
    fn a_ready_host_passes_the_self_test() {
        let root = temp_dir("ready");
        let report = detect_with(&ready_env(&root));
        assert!(report.detected);
        assert!(report.ready, "{:?}", report.checks);
        let test = self_test(&report);
        assert!(test.ready, "{:?}", test.findings);
        assert!(test.findings.is_empty());
    }

    #[test]
    fn every_self_test_message_is_distinct_and_known() {
        let mut seen = BTreeSet::new();
        for code in SELF_TEST_CODES {
            let message = self_test_message(code);
            assert!(!message.is_empty());
            assert!(!message.starts_with("Unknown"));
            assert!(seen.insert(message), "duplicate message for {code}");
        }
        assert_eq!(self_test_message("nope"), "Unknown streaming host finding.");
        assert_eq!(SELF_TEST_CODES.len(), 8);
    }

    #[test]
    fn the_encoder_probe_distinguishes_hardware_and_software() {
        let vainfo = ToolOutput::new(true, "VAProfileH264High : VAEntrypointEncSlice");
        let ffmpeg = ToolOutput::new(true, " V....D h264_vaapi VAAPI H.264");
        let hardware = parse_encoders(Some(&vainfo), Some(&ffmpeg));
        assert!(hardware.available && hardware.hardware);
        let software = parse_encoders(None, Some(&ToolOutput::new(true, " libx264 H.264")));
        assert!(software.available && !software.hardware);
        let none = parse_encoders(None, None);
        assert!(!none.available);
    }

    #[test]
    fn the_plan_is_a_dry_run_and_writes_nothing() {
        let root = temp_dir("plan");
        let env = ready_env(&root);
        let report = detect_with(&env);
        let before = std::fs::read_to_string(root.join("sunshine.conf")).unwrap();
        let plan = plan_config(&report, None, None).unwrap();
        assert!(plan.applicable);
        assert_eq!(plan.output, "DP-1");
        assert_eq!(plan.packet_size, 1200, "wg0 has MTU 1280");
        assert!(!plan.needs_elevation);
        assert_eq!(
            std::fs::read_to_string(root.join("sunshine.conf")).unwrap(),
            before
        );
        assert!(!Path::new(&plan.backup).exists());
        let keys: Vec<&str> = plan.edits.iter().map(|edit| edit.key.as_str()).collect();
        assert_eq!(keys, CONFIG_KEYS);
    }

    #[test]
    fn apply_then_revert_restores_the_original_bytes() {
        let root = temp_dir("apply");
        let env = ready_env(&root);
        let report = detect_with(&env);
        let conf = root.join("sunshine.conf");
        let original = std::fs::read(&conf).unwrap();
        let app_data = root.join("app-data");
        let applied = apply_config(&app_data, &report, None, None, true).unwrap();
        assert!(applied.written);
        assert!(applied.backup.ends_with(".ludex.bak"));
        assert!(Path::new(&applied.backup).is_file());
        let written = std::fs::read_to_string(&conf).unwrap();
        assert!(written.contains("output_name = DP-1"));
        assert!(written.contains("stream_audio = on"));
        assert!(written.contains("packet_size = 1200"));
        assert!(written.contains("# Sunshine configuration"));
        assert!(written.contains("fps = 60"));

        let reverted = revert_config(&app_data).unwrap();
        assert!(reverted.restored);
        assert!(!reverted.removed);
        assert_eq!(std::fs::read(&conf).unwrap(), original);
        assert!(!Path::new(&applied.backup).exists());
        assert!(read_state(&app_data).config.is_none());
    }

    #[test]
    fn apply_creates_a_missing_config_and_revert_removes_it() {
        let root = temp_dir("create");
        let mut env = ready_env(&root);
        let conf = root.join("created/sunshine.conf");
        env.config_candidates = vec![conf.clone()];
        let report = detect_with(&env);
        assert!(!conf.exists());
        let app_data = root.join("app-data");
        apply_config(&app_data, &report, None, None, true).unwrap();
        assert!(conf.is_file());
        let text = std::fs::read_to_string(&conf).unwrap();
        assert!(text.contains("output_name = DP-1"));
        assert!(text.contains("stream_audio = on"));
        let reverted = revert_config(&app_data).unwrap();
        assert!(reverted.removed);
        assert!(!conf.exists());
    }

    #[test]
    fn apply_refuses_without_consent_and_never_replaces_a_mismatched_backup() {
        let root = temp_dir("consent");
        let env = ready_env(&root);
        let report = detect_with(&env);
        let app_data = root.join("app-data");
        let error = apply_config(&app_data, &report, None, None, false).unwrap_err();
        assert_eq!(error.code, "consent_required");
        assert_eq!(
            std::fs::read_to_string(root.join("sunshine.conf")).unwrap(),
            "# Sunshine configuration\nfps = 60\n"
        );

        apply_config(&app_data, &report, None, None, true).unwrap();
        let backup = root.join("sunshine.conf.ludex.bak");
        let saved = std::fs::read(&backup).unwrap();
        // A second apply must keep the pristine backup, not the edited file.
        apply_config(&app_data, &report, None, None, true).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), saved);
    }

    #[test]
    fn the_port_ranges_are_the_documented_sunshine_ports() {
        let rules = sunshine_rules();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].id(), "tcp:47984-47990");
        assert_eq!(rules[1].id(), "tcp:48010-48010");
        assert_eq!(rules[2].id(), "udp:47998-48010");
        assert_eq!(TCP_RANGES, [(47984, 47990), (48010, 48010)]);
        assert_eq!(UDP_RANGES, [(47998, 48010)]);
    }

    #[test]
    fn firewall_commands_name_exact_ports_for_each_tool() {
        let rules = sunshine_rules();
        let ufw = firewall_open_commands(FirewallTool::Ufw, &rules);
        assert_eq!(
            ufw,
            vec![
                "ufw allow 47984:47990/tcp",
                "ufw allow 48010/tcp",
                "ufw allow 47998:48010/udp",
            ]
        );
        let firewalld = firewall_open_commands(FirewallTool::Firewalld, &rules);
        assert_eq!(
            firewalld,
            vec![
                "firewall-cmd --permanent --add-port=47984-47990/tcp",
                "firewall-cmd --permanent --add-port=48010/tcp",
                "firewall-cmd --permanent --add-port=47998-48010/udp",
                "firewall-cmd --reload",
            ]
        );
        let nft = firewall_open_commands(FirewallTool::Nftables, &rules);
        assert_eq!(nft[0], format!("nft add table inet {NFT_TABLE}"));
        assert!(nft
            .iter()
            .any(|line| line.contains("tcp dport { 47984-47990, 48010 } accept")));
        assert!(nft
            .iter()
            .any(|line| line.contains("udp dport { 47998-48010 } accept")));
        for line in ufw.iter().chain(firewalld.iter()).chain(nft.iter()) {
            let lower = line.to_ascii_lowercase();
            assert!(!lower.contains("any"));
            assert!(!lower.contains("default"));
            assert!(!lower.contains("0.0.0.0/0"));
        }
    }

    #[test]
    fn a_revert_only_closes_the_rules_ludex_added() {
        let added = vec!["tcp:47984-47990".to_string()];
        assert_eq!(
            firewall_close_commands(FirewallTool::Ufw, &added, false),
            vec!["ufw delete allow 47984:47990/tcp"]
        );
        assert_eq!(
            firewall_close_commands(FirewallTool::Firewalld, &added, false),
            vec![
                "firewall-cmd --permanent --remove-port=47984-47990/tcp",
                "firewall-cmd --reload",
            ]
        );
        assert!(firewall_close_commands(FirewallTool::Nftables, &added, false).is_empty());
        assert_eq!(
            firewall_close_commands(FirewallTool::Nftables, &added, true),
            vec![format!("nft delete table inet {NFT_TABLE}")]
        );
        // An empty record never invents a rule to delete.
        assert!(firewall_close_commands(FirewallTool::Ufw, &[], false).is_empty());
    }

    #[test]
    fn firewall_presence_is_parsed_per_tool() {
        let ufw = "Status: active\n\nTo                         Action      From\n--                         ------      ----\n47984:47990/tcp            ALLOW       Anywhere\n48010/tcp                  ALLOW       Anywhere\n";
        let present = parse_firewall_presence(FirewallTool::Ufw, ufw);
        assert!(present.contains("tcp:47984-47990"));
        assert!(present.contains("tcp:48010-48010"));
        assert!(!present.contains("udp:47998-48010"));

        let firewalld = "present=tcp:47984-47990\npresent=udp:47998-48010\n";
        let present = parse_firewall_presence(FirewallTool::Firewalld, firewalld);
        assert_eq!(present.len(), 2);

        let nft = format!(
            "table inet {NFT_TABLE} {{\n chain input {{\n  tcp dport {{ 47984-47990, 48010 }} accept\n  udp dport {{ 47998-48010 }} accept\n }}\n}}\n"
        );
        let present = parse_firewall_presence(FirewallTool::Nftables, &nft);
        assert_eq!(present.len(), 3);
        assert!(firewall_table_exists(FirewallTool::Nftables, &nft));
        let partial =
            format!("table inet {NFT_TABLE} {{\n tcp dport {{ 47984-47990, 48010 }} accept\n}}\n");
        assert_eq!(
            parse_firewall_presence(FirewallTool::Nftables, &partial).len(),
            2
        );
        assert!(!firewall_table_exists(FirewallTool::Nftables, ""));
    }

    #[test]
    fn firewall_detection_prefers_the_active_frontend() {
        let ufw = FirewallEnv {
            ufw_binary: true,
            ufw_active: Some(true),
            firewalld_binary: true,
            firewalld_active: true,
            nft_binary: true,
            pkexec: true,
        };
        assert_eq!(detect_firewall(&ufw), Some(FirewallTool::Ufw));
        let firewalld = FirewallEnv {
            ufw_binary: false,
            ufw_active: None,
            firewalld_binary: true,
            firewalld_active: true,
            nft_binary: true,
            pkexec: true,
        };
        assert_eq!(detect_firewall(&firewalld), Some(FirewallTool::Firewalld));
        let none = FirewallEnv {
            ufw_binary: false,
            ufw_active: None,
            firewalld_binary: false,
            firewalld_active: false,
            nft_binary: false,
            pkexec: false,
        };
        assert_eq!(detect_firewall(&none), None);
        let plan = plan_firewall(&none, &StreamHostState::default());
        assert!(!plan.supported);
        assert!(plan.blocked.is_some());
    }

    #[test]
    fn pairing_requests_use_the_documented_endpoints() {
        let request = pin_request("sunshine", "pass", "1234", "Ludex", 47989);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/pin");
        assert_eq!(request.port, 47989);
        assert_eq!(
            request.body,
            serde_json::json!({ "pin": "1234", "name": "Ludex" }).to_string()
        );
        assert!(request.headers.contains(&(
            "Authorization".to_string(),
            "Basic c3Vuc2hpbmU6cGFzcw==".to_string()
        )));
        let apps = apps_request("sunshine", "pass", 47989);
        assert_eq!(apps.method, "GET");
        assert_eq!(apps.path, "/api/apps");
    }

    struct FakeHttp {
        pin: Result<HttpResponse, String>,
        apps: Option<HttpResponse>,
        calls: std::cell::RefCell<Vec<String>>,
    }

    impl PairHttp for FakeHttp {
        fn request(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            self.calls.borrow_mut().push(request.path.clone());
            if request.path == "/api/apps" {
                return Ok(self.apps.clone().unwrap_or(HttpResponse {
                    status: 200,
                    body: r#"{"apps":[]}"#.to_string(),
                }));
            }
            match &self.pin {
                Ok(response) => Ok(response.clone()),
                Err(error) => Err(error.clone()),
            }
        }
    }

    fn fake(pin: Result<HttpResponse, String>) -> FakeHttp {
        FakeHttp {
            pin,
            apps: None,
            calls: std::cell::RefCell::new(Vec::new()),
        }
    }

    #[test]
    fn pairing_outcomes_are_distinct() {
        let accepted = fake(Ok(HttpResponse {
            status: 200,
            body: r#"{"status":"true"}"#.to_string(),
        }));
        let outcome = pair_with(
            &accepted,
            Some("sunshine"),
            Some("pass"),
            "1234",
            "Ludex",
            47989,
        );
        assert_eq!(outcome.mode, "paired");
        assert_eq!(outcome.apps, Some(0));
        assert_eq!(
            accepted.calls.borrow().as_slice(),
            ["/api/pin", "/api/apps"]
        );

        let rejected = fake(Ok(HttpResponse {
            status: 200,
            body: r#"{"status":"false"}"#.to_string(),
        }));
        assert_eq!(
            pair_with(&rejected, Some("u"), Some("p"), "1234", "Ludex", 47989).mode,
            "pin-rejected"
        );

        let unauthorized = fake(Ok(HttpResponse {
            status: 401,
            body: String::new(),
        }));
        assert_eq!(
            pair_with(&unauthorized, Some("u"), Some("p"), "1234", "Ludex", 47989).mode,
            "credentials-required"
        );

        let offline = fake(Err("connection refused".to_string()));
        assert_eq!(
            pair_with(&offline, Some("u"), Some("p"), "1234", "Ludex", 47989).mode,
            "unreachable"
        );

        let no_credentials = fake(Ok(HttpResponse {
            status: 200,
            body: String::new(),
        }));
        assert_eq!(
            pair_with(&no_credentials, None, None, "1234", "Ludex", 47989).mode,
            "credentials-required"
        );

        let bad_pin = fake(Ok(HttpResponse {
            status: 200,
            body: String::new(),
        }));
        assert_eq!(
            pair_with(&bad_pin, Some("u"), Some("p"), "12ab", "Ludex", 47989).mode,
            "invalid-pin"
        );
        assert!(bad_pin.calls.borrow().is_empty());
    }

    #[test]
    fn an_http_response_is_parsed_with_chunked_bodies() {
        let response = parse_http_response(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"a\":\r\n2\r\n1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"a\":1}");
        assert!(parse_http_response("garbage").is_err());
    }

    #[test]
    fn state_roundtrips_through_the_app_data_file() {
        let root = temp_dir("state");
        let state = StreamHostState {
            config: Some(ConfigState {
                path: "/etc/sunshine/sunshine.conf".into(),
                backup: "/etc/sunshine/sunshine.conf.ludex.bak".into(),
                existed: true,
                sha_before: "abc".into(),
                applied_at: "2026-11-19T10:00:00Z".into(),
            }),
            firewall: Some(FirewallState {
                tool: "ufw".into(),
                added: vec!["tcp:47984-47990".into()],
                created_table: false,
                applied_at: "2026-11-19T10:00:00Z".into(),
            }),
        };
        write_state(&root, &state).unwrap();
        assert_eq!(read_state(&root), state);
    }
}
