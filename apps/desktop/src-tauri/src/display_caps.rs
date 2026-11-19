//! Per-output display capabilities (#292).
//!
//! The probes read DRM sysfs only; nothing here opens the DRM device, talks to
//! the compositor or changes a desktop setting. Each connected connector is one
//! record:
//!
//! - VRR from `vrr_capable` (`1` supported, `0` incapable, absent unknown);
//! - HDR from the connector EDID: a CEA-861 extension carrying the HDR Static
//!   Metadata data block means the output advertises HDR, a readable EDID
//!   without that block is incapable, a missing/truncated EDID is unknown;
//! - the highest refresh rate from the EDID range limits or a detailed timing;
//! - the colour depth from the EDID video input definition.
//!
//! The active output is the connected connector whose `enabled` file says
//! `enabled`; when no connector reports that, a single connected output is used
//! so a headless probe still has a target. Tests inject a fixture root, so no
//! test needs a real DRM device.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where Linux exposes DRM connectors.
pub const DEFAULT_DRM_ROOT: &str = "/sys/class/drm";

/// What an output reports for one capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    /// The kernel or EDID advertises the capability.
    Supported,
    /// The kernel or EDID explicitly says the output cannot do it.
    Incapable,
    /// No readable source (missing sysfs attribute or EDID).
    #[default]
    Unknown,
}

/// One connected output and what it reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayOutput {
    /// Connector directory name, e.g. `card0-DP-1`.
    pub connector: String,
    /// Raw `status` file, lower-cased (`connected`).
    pub status: String,
    /// `enabled` says `enabled`, or the only connected output.
    pub active: bool,
    /// `vrr_capable`: supported / incapable / unknown.
    pub vrr: Capability,
    /// EDID HDR static metadata: supported / incapable / unknown.
    pub hdr: Capability,
    /// Highest refresh rate the EDID describes, in Hz.
    pub max_refresh: Option<u32>,
    /// Bits per channel from the EDID video input definition.
    pub color_depth: Option<u8>,
}

/// What one probe learned about the machine's displays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DisplayCapabilities {
    /// Connected outputs only, sorted by connector name.
    pub outputs: Vec<DisplayOutput>,
    /// Connector considered active, when one could be identified.
    pub active: Option<String>,
}

impl DisplayCapabilities {
    /// The active output, falling back to the first connected one.
    pub fn active_output(&self) -> Option<&DisplayOutput> {
        if let Some(active) = self.active.as_deref() {
            if let Some(output) = self
                .outputs
                .iter()
                .find(|output| output.connector == active)
            {
                return Some(output);
            }
        }
        self.outputs.first()
    }

    pub fn vrr(&self) -> Capability {
        self.active_output()
            .map_or(Capability::Unknown, |output| output.vrr)
    }

    pub fn hdr(&self) -> Capability {
        self.active_output()
            .map_or(Capability::Unknown, |output| output.hdr)
    }
}

/* --------------------------------- probe --------------------------------- */

/// Probe one sysfs root (`/sys/class/drm` in production, a fixture in tests).
pub fn probe(root: &Path) -> DisplayCapabilities {
    let Ok(entries) = std::fs::read_dir(root) else {
        return DisplayCapabilities::default();
    };

    let mut outputs: Vec<DisplayOutput> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Only connector nodes: `card0-DP-1`, `card1-HDMI-A-1`. `card0`,
        // `renderD128` and `version` are not outputs.
        if !name.starts_with("card") || !name.contains('-') {
            continue;
        }
        let Some(status) = read_trimmed(&path.join("status")) else {
            continue;
        };
        let status = status.to_ascii_lowercase();
        if status != "connected" {
            continue;
        }

        let vrr = match read_trimmed(&path.join("vrr_capable")).as_deref() {
            Some("1") => Capability::Supported,
            Some("0") => Capability::Incapable,
            _ => Capability::Unknown,
        };
        let edid = read_edid(&path);
        let info = edid.as_deref().map(parse_edid).unwrap_or_default();

        outputs.push(DisplayOutput {
            connector: name.to_string(),
            status,
            // Overwritten below once every connector is known.
            active: false,
            vrr,
            hdr: info.hdr,
            max_refresh: info.max_refresh,
            color_depth: info.color_depth,
        });
    }

    outputs.sort_by(|left, right| left.connector.cmp(&right.connector));

    // `enabled` is the kernel's current routing. Several connectors can be
    // enabled when outputs are mirrored; the first sorted one wins, exactly
    // like the sorted list above.
    let mut active = None;
    for output in &mut outputs {
        let path = root.join(&output.connector).join("enabled");
        if read_trimmed(&path)
            .map(|value| value.eq_ignore_ascii_case("enabled"))
            .unwrap_or(false)
        {
            output.active = true;
            if active.is_none() {
                active = Some(output.connector.clone());
            }
        }
    }
    if active.is_none() && outputs.len() == 1 {
        outputs[0].active = true;
        active = Some(outputs[0].connector.clone());
    }

    DisplayCapabilities { outputs, active }
}

/// Probe the real sysfs root.
pub fn probe_default() -> DisplayCapabilities {
    probe(Path::new(DEFAULT_DRM_ROOT))
}

fn read_trimmed(path: &Path) -> Option<String> {
    let value = std::fs::read_to_string(path).ok()?;
    let value = value.trim().to_ascii_lowercase();
    (!value.is_empty()).then_some(value)
}

/// Read the connector's EDID, if the driver exposes one.
fn read_edid(connector: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(connector.join("edid")).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/* ---------------------------------- EDID --------------------------------- */

const EDID_HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
/// Where the four 18-byte descriptors start in a base block.
const DESCRIPTOR_START: usize = 0x36;
/// CEA-861 extension tag.
const CEA_EXTENSION: u8 = 0x02;
/// HDR Static Metadata extended data block tag (CEA-861-F).
const HDR_STATIC_METADATA: u8 = 0x06;

/// The parts of an EDID this probe reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EdidInfo {
    pub hdr: Capability,
    pub max_refresh: Option<u32>,
    pub color_depth: Option<u8>,
}

/// Parse a base EDID block plus its CEA extensions.
///
/// Only capabilities are extracted: HDR needs the HDR Static Metadata block in
/// a CEA-861 extension, the refresh rate is the highest vertical rate the range
/// limits or a detailed timing describe, and the colour depth is the digital
/// bit depth field of the video input definition. Everything else stays
/// unread, so a monitor quirk never turns into a fabricated capability.
pub fn parse_edid(bytes: &[u8]) -> EdidInfo {
    if bytes.len() < 128 || bytes[..8] != EDID_HEADER {
        return EdidInfo::default();
    }

    // Bit 7 is the digital input flag; bits 6–4 are the bit depth, and code 0
    // means "undefined". An analogue input has no encoded depth at all.
    let color_depth = if bytes[0x14] & 0x80 == 0 {
        None
    } else {
        match (bytes[0x14] >> 4) & 0x07 {
            1 => Some(6),
            2 => Some(8),
            3 => Some(10),
            4 => Some(12),
            5 => Some(14),
            6 => Some(16),
            _ => None,
        }
    };

    let mut max_refresh: Option<u32> = None;
    for index in 0..4 {
        let descriptor = DESCRIPTOR_START + index * 18;
        let clock_10khz = u32::from(bytes[descriptor]) | (u32::from(bytes[descriptor + 1]) << 8);
        if clock_10khz > 0 {
            if let Some(refresh) = detailed_timing_refresh(bytes, descriptor, clock_10khz) {
                max_refresh = Some(max_refresh.map_or(refresh, |current| current.max(refresh)));
            }
            continue;
        }
        // `0xFD` = display range limits: bytes 6 and 8 are the maximum vertical
        // and horizontal rates.
        if bytes[descriptor + 2] == 0xFD {
            let vertical = u32::from(bytes[descriptor + 6]);
            if vertical > 0 {
                max_refresh = Some(max_refresh.map_or(vertical, |current| current.max(vertical)));
            }
        }
    }

    let extension_count = usize::from(bytes[0x7E]);
    let mut hdr = Capability::Incapable;
    for index in 0..extension_count {
        let start = 128 * (index + 1);
        let Some(block) = bytes.get(start..start + 128) else {
            // The EDID claims an extension that was not exposed: without it
            // nothing can be asserted about HDR.
            hdr = Capability::Unknown;
            break;
        };
        if block[0] != CEA_EXTENSION {
            continue;
        }
        if cea_has_hdr_metadata(block) {
            hdr = Capability::Supported;
        }
    }

    EdidInfo {
        hdr,
        max_refresh,
        color_depth,
    }
}

/// `true` when the CEA Data Block Collection carries a HDR Static Metadata
/// Data Block (extended tag `0x06`).
fn cea_has_hdr_metadata(block: &[u8]) -> bool {
    let dtd_offset = usize::from(block[2]);
    let end = if (4..=127).contains(&dtd_offset) {
        dtd_offset
    } else {
        127
    };
    let mut cursor = 4;
    while cursor < end {
        let tag = block[cursor] >> 5;
        let length = usize::from(block[cursor] & 0x1F);
        if length == 0 || cursor + 1 + length > block.len() {
            break;
        }
        if tag == 7 && block[cursor + 1] == HDR_STATIC_METADATA {
            return true;
        }
        cursor += 1 + length;
    }
    false
}

/// Refresh rate of one 18-byte detailed timing descriptor, rounded to Hz.
fn detailed_timing_refresh(bytes: &[u8], offset: usize, clock_10khz: u32) -> Option<u32> {
    let h_active = u32::from(bytes[offset + 2]) | (u32::from(bytes[offset + 4] & 0xF0) << 4);
    let h_blank = u32::from(bytes[offset + 3]) | (u32::from(bytes[offset + 4] & 0x0F) << 8);
    let v_active = u32::from(bytes[offset + 5]) | (u32::from(bytes[offset + 7] & 0xF0) << 4);
    let v_blank = u32::from(bytes[offset + 6]) | (u32::from(bytes[offset + 7] & 0x0F) << 8);
    let total = (h_active + h_blank) * (v_active + v_blank);
    if total == 0 {
        return None;
    }
    let refresh = (f64::from(clock_10khz) * 10_000.0) / f64::from(total);
    (refresh.is_finite() && refresh > 0.0).then(|| refresh.round() as u32)
}

/* --------------------------------- tests --------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ludex-display-caps-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, value: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, value).unwrap();
    }

    fn connector(root: &Path, name: &str, status: &str) {
        write(&root.join(name).join("status"), status);
        write(&root.join(name).join("enabled"), "disabled");
    }

    /// A base EDID: 10-bit digital input, range limits 48–240 Hz.
    fn base_edid() -> Vec<u8> {
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&EDID_HEADER);
        edid[0x14] = 0xB0; // digital input, 10 bits per channel
        let descriptor = DESCRIPTOR_START;
        edid[descriptor + 2] = 0xFD;
        edid[descriptor + 5] = 48;
        edid[descriptor + 6] = 240;
        edid[descriptor + 7] = 30;
        edid[descriptor + 8] = 255;
        edid
    }

    /// A CEA-861 extension carrying the HDR Static Metadata block.
    fn cea_extension(hdr: bool) -> Vec<u8> {
        let mut block = vec![0u8; 128];
        block[0] = CEA_EXTENSION;
        block[1] = 0x03;
        if hdr {
            block[2] = 10; // DTDs start after the six-byte HDR block
            block[4] = (7 << 5) | 5; // extended data block, five bytes
            block[5] = HDR_STATIC_METADATA;
            block[6] = 0x01; // SDR + HDR
            block[7] = 0x01; // static metadata type 1
            block[8] = 0;
            block[9] = 0;
        } else {
            block[2] = 8;
            block[4] = (2 << 5) | 3; // video data block, no HDR
            block[5] = 0x04;
            block[6] = 0x13;
            block[7] = 0x1F;
        }
        block
    }

    fn edid_with(hdr: bool) -> Vec<u8> {
        let mut edid = base_edid();
        edid[0x7E] = 1;
        edid.extend_from_slice(&cea_extension(hdr));
        edid
    }

    #[test]
    fn an_hdr_edid_reports_the_static_metadata_block() {
        let info = parse_edid(&edid_with(true));
        assert_eq!(info.hdr, Capability::Supported);
        assert_eq!(info.max_refresh, Some(240));
        assert_eq!(info.color_depth, Some(10));
    }

    #[test]
    fn a_readable_edid_without_the_hdr_block_is_incapable() {
        let info = parse_edid(&edid_with(false));
        assert_eq!(info.hdr, Capability::Incapable);

        // No extension at all: the monitor cannot advertise HDR metadata.
        let info = parse_edid(&base_edid());
        assert_eq!(info.hdr, Capability::Incapable);
    }

    #[test]
    fn an_unreadable_edid_is_unknown() {
        assert_eq!(parse_edid(&[]).hdr, Capability::Unknown);
        assert_eq!(parse_edid(&[0u8; 64]).hdr, Capability::Unknown);
        // A declared extension the driver did not expose stays unknown.
        let mut truncated = base_edid();
        truncated[0x7E] = 1;
        assert_eq!(parse_edid(&truncated).hdr, Capability::Unknown);
    }

    #[test]
    fn the_highest_refresh_comes_from_the_range_limits_or_a_detailed_timing() {
        // Range limits only.
        assert_eq!(parse_edid(&base_edid()).max_refresh, Some(240));

        // A 1920×1080@60 detailed timing in the second descriptor slot.
        let mut edid = base_edid();
        for byte in &mut edid[DESCRIPTOR_START..DESCRIPTOR_START + 18] {
            *byte = 0;
        }
        let dtd = DESCRIPTOR_START + 18;
        edid[dtd] = 0x02; // 148.5 MHz in 10 kHz units
        edid[dtd + 1] = 0x3A;
        edid[dtd + 2] = 0x80; // 1920
        edid[dtd + 3] = 0x18; // 280 blanking
        edid[dtd + 4] = 0x71;
        edid[dtd + 5] = 0x38; // 1080
        edid[dtd + 6] = 0x2D; // 45 lines of blanking
        edid[dtd + 7] = 0x40;
        assert_eq!(parse_edid(&edid).max_refresh, Some(60));
    }

    #[test]
    fn vrr_capability_reads_zero_one_and_missing() {
        let root = fixture("vrr");
        connector(&root, "card0-DP-1", "connected");
        write(&root.join("card0-DP-1/vrr_capable"), "1");
        connector(&root, "card0-HDMI-A-1", "connected");
        write(&root.join("card0-HDMI-A-1/vrr_capable"), "0");
        connector(&root, "card0-DP-2", "connected");

        let caps = probe(&root);
        assert_eq!(caps.outputs.len(), 3);
        let by_name = |name: &str| {
            caps.outputs
                .iter()
                .find(|output| output.connector == name)
                .unwrap()
        };
        assert_eq!(by_name("card0-DP-1").vrr, Capability::Supported);
        assert_eq!(by_name("card0-HDMI-A-1").vrr, Capability::Incapable);
        assert_eq!(by_name("card0-DP-2").vrr, Capability::Unknown);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_edid_of_each_connector_decides_hdr() {
        let root = fixture("edid");
        connector(&root, "card0-DP-1", "connected");
        fs::write(root.join("card0-DP-1/edid"), edid_with(true)).unwrap();
        connector(&root, "card0-HDMI-A-1", "connected");
        fs::write(root.join("card0-HDMI-A-1/edid"), edid_with(false)).unwrap();
        connector(&root, "card0-DP-2", "connected");

        let caps = probe(&root);
        let by_name = |name: &str| {
            caps.outputs
                .iter()
                .find(|output| output.connector == name)
                .unwrap()
        };
        assert_eq!(by_name("card0-DP-1").hdr, Capability::Supported);
        assert_eq!(by_name("card0-DP-1").max_refresh, Some(240));
        assert_eq!(by_name("card0-DP-1").color_depth, Some(10));
        assert_eq!(by_name("card0-HDMI-A-1").hdr, Capability::Incapable);
        assert_eq!(by_name("card0-DP-2").hdr, Capability::Unknown);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_connected_outputs_are_reported() {
        let root = fixture("connected");
        connector(&root, "card0-DP-1", "connected");
        write(&root.join("card0-DP-1/enabled"), "enabled");
        connector(&root, "card0-DP-2", "disconnected");
        write(&root.join("card0-DP-2/vrr_capable"), "1");

        let caps = probe(&root);
        assert_eq!(caps.outputs.len(), 1);
        assert_eq!(caps.outputs[0].connector, "card0-DP-1");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_active_output_is_the_enabled_connector() {
        let root = fixture("active");
        connector(&root, "card0-DP-1", "connected");
        write(&root.join("card0-DP-1/enabled"), "enabled");
        write(&root.join("card0-DP-1/vrr_capable"), "1");
        connector(&root, "card0-HDMI-A-1", "connected");
        write(&root.join("card0-HDMI-A-1/vrr_capable"), "0");

        let caps = probe(&root);
        assert_eq!(caps.active.as_deref(), Some("card0-DP-1"));
        assert!(caps.outputs[0].active);
        assert!(!caps.outputs[1].active);
        assert_eq!(caps.vrr(), Capability::Supported);

        // No `enabled` on a single connected output: it is the target anyway.
        let root = fixture("active-single");
        connector(&root, "card1-DP-1", "connected");
        write(&root.join("card1-DP-1/vrr_capable"), "0");
        let caps = probe(&root);
        assert_eq!(caps.active.as_deref(), Some("card1-DP-1"));
        assert_eq!(caps.vrr(), Capability::Incapable);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_sysfs_root_reports_nothing_instead_of_failing() {
        let caps = probe(Path::new("/nonexistent/ludex/drm"));
        assert!(caps.outputs.is_empty());
        assert!(caps.active.is_none());
        assert_eq!(caps.vrr(), Capability::Unknown);
        assert_eq!(caps.hdr(), Capability::Unknown);
    }

    #[test]
    fn the_serialised_shape_is_camel_case() {
        let caps = DisplayCapabilities {
            outputs: vec![DisplayOutput {
                connector: "card0-DP-1".to_string(),
                status: "connected".to_string(),
                active: true,
                vrr: Capability::Supported,
                hdr: Capability::Incapable,
                max_refresh: Some(240),
                color_depth: Some(10),
            }],
            active: Some("card0-DP-1".to_string()),
        };
        let json = serde_json::to_value(&caps).unwrap();
        assert_eq!(json["active"], "card0-DP-1");
        assert_eq!(json["outputs"][0]["maxRefresh"], 240);
        assert_eq!(json["outputs"][0]["colorDepth"], 10);
        assert_eq!(json["outputs"][0]["vrr"], "supported");
        assert_eq!(json["outputs"][0]["hdr"], "incapable");
    }
}
