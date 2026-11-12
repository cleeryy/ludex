//! Remote play wizard (#179): safe UPnP port mapping for Sunshine and
//! Moonlight, with a VPN-first recommendation.
//!
//! The module never bundles a relay and never edits Sunshine's configuration.
//! It reads `sunshine.conf` to report what Sunshine already does, discovers a
//! UPnP IGD on the LAN, and maps only the ports Moonlight needs — after an
//! explicit consent screen that lists the exact ports, protocol and lease.
//!
//! Safety rules encoded here:
//! - TCP 47990 (Sunshine Web UI) and UDP 5353 (mDNS) are never mapped.
//! - A mapping is refused when the IGD is not on a private LAN address, when
//!   the WAN address is CGNAT or private (double NAT), or when Sunshine is
//!   allowed to expose its Web UI/PIN over the WAN.
//! - Ports already owned by another device or a non-Ludex mapping are refused.
//! - Every mapping is written with a short lease and recorded locally so a
//!   crash, a cancel or the next startup can remove it.
//! - No credential, PIN or token is read or logged.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::LocalSettings;
use crate::streaming;

/* ------------------------------ constants -------------------------------- */

/// TCP ports Moonlight needs: HTTPS/NVHTTP, HTTP pairing, RTSP.
pub const TCP_PORTS: [u16; 3] = [47984, 47989, 48010];
/// UDP ports Moonlight needs: video, control, audio, RTSP.
pub const UDP_PORTS: [u16; 4] = [47998, 47999, 48000, 48010];
/// Optional microphone stream, only mapped when the user asks for it.
pub const OPTIONAL_UDP_PORT: u16 = 48002;
/// Never forwarded: Sunshine's admin/pairing Web UI and mDNS (#179).
pub const FORBIDDEN_PORTS: [u16; 2] = [47990, 5353];
/// Default mapping lease, refreshed before it expires.
pub const DEFAULT_LEASE_SECS: u64 = 3600;
/// A lease shorter than this would expire mid-session; longer ones defeat the
/// automatic cleanup guarantee.
pub const MIN_LEASE_SECS: u64 = 600;
pub const MAX_LEASE_SECS: u64 = 7200;
/// Hard cap on the number of forwarded ports: the plan only ever contains 8
/// (9 with the microphone), so anything above this means a bug.
pub const MAX_REMOTE_PORTS: usize = 9;
/// Description prefix that marks a mapping as Ludex-owned on the router.
pub const MAPPING_PREFIX: &str = "Ludex remote play";

/* -------------------------------- errors --------------------------------- */

/// Structured command error. Tauri serializes this object as the rejected
/// value; the web UI reads `code` to pick the right explanation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlayError {
    pub code: String,
    pub message: String,
}

impl RemotePlayError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RemotePlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RemotePlayError {}

/* ------------------------------- port plan -------------------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortProtocol {
    Tcp,
    Udp,
}

impl PortProtocol {
    pub fn soap(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

impl std::fmt::Display for PortProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.soap())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortPlan {
    pub tcp: Vec<u16>,
    pub udp: Vec<u16>,
    pub lease_secs: u64,
    pub microphone: bool,
    pub total: usize,
    /// Ports that exist in Sunshine but are deliberately not forwarded.
    pub never_mapped: Vec<u16>,
    pub warnings: Vec<String>,
}

/// The exact list of ports the wizard may map. Pure and total: callers cannot
/// add ports, and the forbidden ports are excluded by construction.
pub fn plan_ports(microphone: bool, lease_secs: Option<u64>) -> PortPlan {
    let lease_secs = lease_secs
        .unwrap_or(DEFAULT_LEASE_SECS)
        .clamp(MIN_LEASE_SECS, MAX_LEASE_SECS);
    let tcp = TCP_PORTS.to_vec();
    let mut udp = UDP_PORTS.to_vec();
    if microphone {
        udp.push(OPTIONAL_UDP_PORT);
    }
    udp.sort_unstable();
    PortPlan {
        total: tcp.len() + udp.len(),
        tcp,
        udp,
        lease_secs,
        microphone,
        never_mapped: FORBIDDEN_PORTS.to_vec(),
        warnings: vec![
            "These ports are reachable from the internet while the mapping exists. Sunshine's PIN and TLS are the only authentication.".to_string(),
            "The Sunshine Web UI (TCP 47990) and mDNS (UDP 5353) are never forwarded; keep the Web UI on the LAN.".to_string(),
            "UPnP and Moonlight add bandwidth and latency to the host's connection, and some games or anti-cheat systems do not tolerate remote streaming. Check the game's terms.".to_string(),
        ],
    }
}

/// Defense in depth: the mapping command accepts only the canonical plan.
pub fn validate_plan(plan: &PortPlan) -> Result<(), RemotePlayError> {
    if plan.lease_secs < MIN_LEASE_SECS || plan.lease_secs > MAX_LEASE_SECS {
        return Err(RemotePlayError::new(
            "lease_invalid",
            format!(
                "The mapping lease must be between {MIN_LEASE_SECS} and {MAX_LEASE_SECS} seconds."
            ),
        ));
    }
    if plan.total == 0 || plan.total > MAX_REMOTE_PORTS {
        return Err(RemotePlayError::new(
            "port_plan_invalid",
            "The port plan is empty or exposes more ports than Moonlight needs.",
        ));
    }
    for port in &plan.tcp {
        if !TCP_PORTS.contains(port) || FORBIDDEN_PORTS.contains(port) {
            return Err(RemotePlayError::new(
                "port_plan_invalid",
                format!("TCP {port} is not a Moonlight port Ludex may forward."),
            ));
        }
    }
    for port in &plan.udp {
        let allowed = UDP_PORTS.contains(port) || (plan.microphone && *port == OPTIONAL_UDP_PORT);
        if !allowed || FORBIDDEN_PORTS.contains(port) {
            return Err(RemotePlayError::new(
                "port_plan_invalid",
                format!("UDP {port} is not a Moonlight port Ludex may forward."),
            ));
        }
    }
    let mut tcp = BTreeSet::new();
    for port in &plan.tcp {
        if !tcp.insert(*port) {
            return Err(RemotePlayError::new(
                "port_plan_invalid",
                format!("TCP {port} is listed twice."),
            ));
        }
    }
    let mut udp = BTreeSet::new();
    for port in &plan.udp {
        if !udp.insert(*port) {
            return Err(RemotePlayError::new(
                "port_plan_invalid",
                format!("UDP {port} is listed twice."),
            ));
        }
    }
    if plan.total != plan.tcp.len() + plan.udp.len() {
        return Err(RemotePlayError::new(
            "port_plan_invalid",
            "The port plan count does not match the listed ports.",
        ));
    }
    Ok(())
}

/* ------------------------------- mappings --------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortMapping {
    pub protocol: PortProtocol,
    pub external_port: u16,
    pub internal_port: u16,
    pub internal_client: String,
    pub lease_secs: u64,
    pub description: String,
}

/// A mapping Ludex added and recorded locally, with its expiry for refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveMapping {
    pub protocol: PortProtocol,
    pub external_port: u16,
    pub internal_port: u16,
    pub internal_client: String,
    pub lease_secs: u64,
    pub description: String,
    /// Unix seconds. The router drops the mapping at or after this time.
    pub expires_at: i64,
}

impl ActiveMapping {
    pub fn from_mapping(mapping: &PortMapping, now: i64) -> Self {
        Self {
            protocol: mapping.protocol,
            external_port: mapping.external_port,
            internal_port: mapping.internal_port,
            internal_client: mapping.internal_client.clone(),
            lease_secs: mapping.lease_secs,
            description: mapping.description.clone(),
            expires_at: now.saturating_add(mapping.lease_secs as i64),
        }
    }

    pub fn to_mapping(&self) -> PortMapping {
        PortMapping {
            protocol: self.protocol,
            external_port: self.external_port,
            internal_port: self.internal_port,
            internal_client: self.internal_client.clone(),
            lease_secs: self.lease_secs,
            description: self.description.clone(),
        }
    }
}

fn mapping_description(protocol: PortProtocol, port: u16) -> String {
    // UPnP descriptions are capped at 64 bytes by many routers.
    format!("{MAPPING_PREFIX} {protocol} {port}")
}

pub fn is_ludex_mapping(description: &str) -> bool {
    description.starts_with(MAPPING_PREFIX)
}

/// One mapping per planned port, targeting the detected LAN address.
pub fn plan_mappings(plan: &PortPlan, internal_client: &str) -> Vec<PortMapping> {
    let mut out = Vec::with_capacity(plan.total);
    for port in &plan.tcp {
        out.push(PortMapping {
            protocol: PortProtocol::Tcp,
            external_port: *port,
            internal_port: *port,
            internal_client: internal_client.to_string(),
            lease_secs: plan.lease_secs,
            description: mapping_description(PortProtocol::Tcp, *port),
        });
    }
    for port in &plan.udp {
        out.push(PortMapping {
            protocol: PortProtocol::Udp,
            external_port: *port,
            internal_port: *port,
            internal_client: internal_client.to_string(),
            lease_secs: plan.lease_secs,
            description: mapping_description(PortProtocol::Udp, *port),
        });
    }
    out
}

/* ----------------------------- SOAP builders ------------------------------ */

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn soap_envelope(service_type: &str, action: &str, arguments: &str) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><u:{action} xmlns:u=\"{service_type}\">{arguments}</u:{action}>\
         </s:Body></s:Envelope>"
    )
}

pub fn add_port_mapping_body(service_type: &str, mapping: &PortMapping) -> String {
    let arguments = format!(
        "<NewRemoteHost></NewRemoteHost>\
         <NewExternalPort>{}</NewExternalPort>\
         <NewProtocol>{}</NewProtocol>\
         <NewInternalPort>{}</NewInternalPort>\
         <NewInternalClient>{}</NewInternalClient>\
         <NewEnabled>1</NewEnabled>\
         <NewPortMappingDescription>{}</NewPortMappingDescription>\
         <NewLeaseDuration>{}</NewLeaseDuration>",
        mapping.external_port,
        mapping.protocol.soap(),
        mapping.internal_port,
        xml_escape(&mapping.internal_client),
        xml_escape(&mapping.description),
        mapping.lease_secs
    );
    soap_envelope(service_type, "AddPortMapping", &arguments)
}

pub fn delete_port_mapping_body(service_type: &str, protocol: PortProtocol, port: u16) -> String {
    let arguments = format!(
        "<NewRemoteHost></NewRemoteHost>\
         <NewExternalPort>{port}</NewExternalPort>\
         <NewProtocol>{}</NewProtocol>",
        protocol.soap()
    );
    soap_envelope(service_type, "DeletePortMapping", &arguments)
}

pub fn get_external_ip_body(service_type: &str) -> String {
    soap_envelope(service_type, "GetExternalIPAddress", "")
}

pub fn get_specific_port_mapping_body(
    service_type: &str,
    protocol: PortProtocol,
    port: u16,
) -> String {
    let arguments = format!(
        "<NewRemoteHost></NewRemoteHost>\
         <NewExternalPort>{port}</NewExternalPort>\
         <NewProtocol>{}</NewProtocol>",
        protocol.soap()
    );
    soap_envelope(service_type, "GetSpecificPortMappingEntry", &arguments)
}

/* ------------------------------- XML parsing ------------------------------ */

/// Text of the first `<tag>…</tag>` element, namespace tolerant. An empty or
/// self-closing element returns an empty string.
pub fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let after = &xml[start + open.len()..];
    let gt = after.find('>')?;
    if after[..gt].trim_end().ends_with('/') {
        return Some(String::new());
    }
    let body = &after[gt + 1..];
    let close = format!("</{tag}>");
    let end = body.find(&close)?;
    Some(body[..end].trim().to_string())
}

/// Blocks of `<tag>…</tag>`, tolerant of attributes and namespace prefixes.
fn xml_blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start..];
        let Some(gt) = after.find('>') else { break };
        let body = &after[gt + 1..];
        let Some(end) = body.find(&close) else { break };
        out.push(&body[..end]);
        rest = &body[end + close.len()..];
    }
    out
}

pub fn parse_external_ip(xml: &str) -> Option<IpAddr> {
    tag_text(xml, "NewExternalIPAddress").and_then(|value| value.parse::<IpAddr>().ok())
}

/// SOAP faults carry an `UPnPError` with a numeric `errorCode`. 714 means the
/// requested entry does not exist, which is the normal "not mapped" answer.
pub fn parse_fault_code(xml: &str) -> Option<u32> {
    tag_text(xml, "errorCode").and_then(|value| value.parse::<u32>().ok())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMapping {
    pub internal_client: String,
    pub internal_port: u16,
    pub description: String,
    pub enabled: bool,
    pub lease_secs: u64,
}

pub fn parse_mapping_entry(xml: &str) -> Option<RemoteMapping> {
    let internal_client = tag_text(xml, "NewInternalClient")?;
    let internal_port = tag_text(xml, "NewInternalPort")?.parse::<u16>().ok()?;
    let enabled = tag_text(xml, "NewEnabled")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(true);
    Some(RemoteMapping {
        internal_client,
        internal_port,
        description: tag_text(xml, "NewPortMappingDescription").unwrap_or_default(),
        enabled,
        lease_secs: tag_text(xml, "NewLeaseDuration")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
    })
}

/* ---------------------------------- IGD ----------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IgdDevice {
    /// SSDP `LOCATION`: the device description URL.
    pub location: String,
    /// Absolute control URL for the WAN connection service.
    pub control_url: String,
    pub service_type: String,
    pub friendly_name: String,
    pub model: String,
}

/// Resolve a control URL against the description URL.
pub fn resolve_url(base: &str, relative: &str) -> String {
    let relative = relative.trim();
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return relative.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let (authority, base_path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if relative.starts_with('/') {
        return format!("{scheme}://{authority}{relative}");
    }
    let directory = match base_path.rfind('/') {
        Some(index) => &base_path[..=index],
        None => "/",
    };
    format!("{scheme}://{authority}{directory}{relative}")
}

pub fn parse_igd_description(xml: &str, location: &str) -> Option<IgdDevice> {
    let mut fallback: Option<IgdDevice> = None;
    for block in xml_blocks(xml, "service") {
        let Some(service_type) = tag_text(block, "serviceType") else {
            continue;
        };
        let is_ip = service_type.contains("WANIPConnection");
        let is_ppp = service_type.contains("WANPPPConnection");
        if !is_ip && !is_ppp {
            continue;
        }
        let Some(control) = tag_text(block, "controlURL") else {
            continue;
        };
        let device = IgdDevice {
            location: location.to_string(),
            control_url: resolve_url(location, &control),
            service_type,
            friendly_name: tag_text(xml, "friendlyName").unwrap_or_default(),
            model: tag_text(xml, "modelName").unwrap_or_default(),
        };
        if is_ip {
            return Some(device);
        }
        fallback.get_or_insert(device);
    }
    fallback
}

/// SSDP `LOCATION` header from a discovery response.
pub fn parse_ssdp_location(packet: &str) -> Option<String> {
    packet.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("location") {
            let value = value.trim();
            if value.starts_with("http://") {
                return Some(value.to_string());
            }
        }
        None
    })
}

pub fn is_private_ip(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

pub fn is_private_ipv4(addr: Ipv4Addr) -> bool {
    addr.is_private() || addr.is_link_local() || addr.is_loopback()
}

pub fn is_private_ipv6(addr: Ipv6Addr) -> bool {
    is_ula_ipv6(addr) || is_link_local_ipv6(addr)
}

/// fc00::/7 unique-local: private, but not reachable from another network.
pub fn is_ula_ipv6(addr: Ipv6Addr) -> bool {
    (addr.segments()[0] & 0xfe00) == 0xfc00
}

/// fe80::/10 link-local: never routed, so never advertised (#206).
pub fn is_link_local_ipv6(addr: Ipv6Addr) -> bool {
    (addr.segments()[0] & 0xffc0) == 0xfe80
}

/// A UPnP control endpoint must be on the LAN. A public `LOCATION` means an
/// ISP-managed or hostile IGD, which is refused rather than trusted.
pub fn location_is_private(location: &str) -> bool {
    let Some((host, _port)) = http_host_port(location) else {
        return false;
    };
    if let Ok(addr) = host.parse::<IpAddr>() {
        return is_private_ip(addr);
    }
    // Hostnames are resolved and every address must be private; a hostname
    // that does not resolve is not trusted.
    let Ok(addresses) = (host.as_str(), 80u16).to_socket_addrs() else {
        return false;
    };
    let mut saw_any = false;
    for address in addresses {
        saw_any = true;
        if !is_private_ip(address.ip()) {
            return false;
        }
    }
    saw_any
}

fn http_host_port(url: &str) -> Option<(String, u16)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?.trim();
    if authority.is_empty() {
        return None;
    }
    // IPv6 literal in brackets.
    if let Some(end) = authority.find(']') {
        let host = authority[..=end]
            .trim_start_matches('[')
            .trim_end_matches(']');
        let port = authority[end + 1..]
            .strip_prefix(':')
            .and_then(|value| value.parse().ok())
            .unwrap_or(80);
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => Some((host.to_string(), port.parse().ok()?)),
        None => Some((authority.to_string(), 80)),
    }
}

/* ----------------------------- HTTP transport ----------------------------- */

pub trait IgdTransport {
    fn get(&self, url: &str) -> Result<String, String>;
    fn post(&self, url: &str, soap_action: &str, body: &str) -> Result<String, String>;
}

/// Minimal HTTP/1.1 client for IGD control endpoints.
///
/// IGD control URLs are plain HTTP on the LAN. A full HTTP client is avoided on
/// purpose: no TLS stack, no redirects, no cookies, a hard response cap and a
/// fixed timeout. An `https://` control URL is refused with an explanation.
pub struct HttpTransport {
    timeout: Duration,
}

impl HttpTransport {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    fn request(
        &self,
        method: &str,
        url: &str,
        soap_action: Option<&str>,
        body: Option<&str>,
    ) -> Result<String, String> {
        if url.starts_with("https://") {
            return Err(
                "The router only offers an HTTPS UPnP control URL, which Ludex does not support."
                    .to_string(),
            );
        }
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("Unsupported UPnP URL: {url}"))?;
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = http_host_port(url).ok_or_else(|| format!("Invalid UPnP URL: {url}"))?;
        let address = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|error| format!("Could not resolve {host}: {error}"))?
            .next()
            .ok_or_else(|| format!("Could not resolve {host}"))?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)
            .map_err(|error| format!("The router did not answer at {authority}: {error}"))?;
        let _ = stream.set_read_timeout(Some(self.timeout));
        let _ = stream.set_write_timeout(Some(self.timeout));

        let body = body.unwrap_or("");
        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n");
        if let Some(action) = soap_action {
            request.push_str("Content-Type: text/xml; charset=\"utf-8\"\r\n");
            request.push_str(&format!("SOAPAction: \"{action}\"\r\n"));
        }
        if method.eq_ignore_ascii_case("POST") {
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        request.push_str(body);
        stream
            .write_all(request.as_bytes())
            .map_err(|error| format!("Could not send the UPnP request: {error}"))?;

        let mut raw = Vec::new();
        stream
            .take(256 * 1024)
            .read_to_end(&mut raw)
            .map_err(|error| format!("Could not read the UPnP response: {error}"))?;
        let text = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or(("", text.as_str()));
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(0);
        // SOAP faults use HTTP 500 but carry the UPnP errorCode in the body;
        // callers parse it instead of treating every 500 as a transport error.
        if status == 0 || (status >= 400 && !body.contains("UPnPError")) {
            return Err(format!(
                "The router rejected the UPnP request (HTTP {status})."
            ));
        }
        Ok(body.to_string())
    }
}

impl IgdTransport for HttpTransport {
    fn get(&self, url: &str) -> Result<String, String> {
        self.request("GET", url, None, None)
    }

    fn post(&self, url: &str, soap_action: &str, body: &str) -> Result<String, String> {
        self.request("POST", url, Some(soap_action), Some(body))
    }
}

/// SSDP IGD search. Bounded by `timeout`, never blocks longer than ~2× it.
pub fn ssdp_locations(timeout: Duration) -> Vec<String> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else {
        return Vec::new();
    };
    let _ = socket.set_read_timeout(Some(Duration::from_millis(400)));
    let message = "M-SEARCH * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 2\r\n\
         ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n";
    let _ = socket.send_to(message.as_bytes(), "239.255.255.250:1900");

    let deadline = Instant::now() + timeout;
    let mut locations = Vec::new();
    let mut buffer = [0u8; 4096];
    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((size, _)) => {
                let packet = String::from_utf8_lossy(&buffer[..size]);
                if let Some(location) = parse_ssdp_location(&packet) {
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
            }
            Err(_) => continue,
        }
    }
    locations
}

/// Discover the first private IGD and its WAN connection service.
pub fn discover_igd(
    transport: &dyn IgdTransport,
    timeout: Duration,
) -> Result<IgdDevice, RemotePlayError> {
    let locations = ssdp_locations(timeout);
    if locations.is_empty() {
        return Err(RemotePlayError::new(
            "upnp_unavailable",
            "No UPnP gateway answered the discovery request. Enable UPnP-IGD on the router, or use a VPN instead.",
        ));
    }
    let mut public_seen = false;
    let mut last_error = String::new();
    for location in locations {
        if !location_is_private(&location) {
            public_seen = true;
            continue;
        }
        match transport.get(&location) {
            Ok(xml) => {
                if let Some(device) = parse_igd_description(&xml, &location) {
                    return Ok(device);
                }
                last_error = format!("{location} did not describe a WAN connection service");
            }
            Err(error) => last_error = error,
        }
    }
    if public_seen {
        return Err(RemotePlayError::new(
            "public_igd",
            "The only UPnP gateway Ludex found is not on the local network. Mapping through a remote IGD is refused.",
        ));
    }
    Err(RemotePlayError::new(
        "upnp_unavailable",
        format!("The router answered UPnP discovery but no usable WAN connection was found. {last_error}"),
    ))
}

/* ----------------------------- gateway client ----------------------------- */

pub trait PortMapper {
    fn external_ip(&self) -> Result<IpAddr, RemotePlayError>;
    fn add(&self, mapping: &PortMapping) -> Result<(), RemotePlayError>;
    fn delete(&self, protocol: PortProtocol, port: u16) -> Result<(), RemotePlayError>;
    fn get(
        &self,
        protocol: PortProtocol,
        port: u16,
    ) -> Result<Option<RemoteMapping>, RemotePlayError>;
}

pub struct UpnpGateway<T: IgdTransport> {
    pub device: IgdDevice,
    pub transport: T,
}

impl<T: IgdTransport> UpnpGateway<T> {
    pub fn new(device: IgdDevice, transport: T) -> Self {
        Self { device, transport }
    }

    fn action_header(&self, action: &str) -> String {
        format!("\"{}#{action}\"", self.device.service_type)
    }

    fn call(&self, action: &str, body: &str) -> Result<String, RemotePlayError> {
        self.transport
            .post(&self.device.control_url, &self.action_header(action), body)
            .map_err(|message| RemotePlayError::new("upnp_unavailable", message))
    }

    fn fault(error_code: u32, action: &str) -> RemotePlayError {
        let hint = match error_code {
            718 => "another mapping already uses this port",
            724 => "the router does not support this mapping protocol",
            725 => "the router only supports permanent leases",
            501 | 401 => "the router does not implement this action",
            _ => "the router rejected the mapping",
        };
        RemotePlayError::new(
            "upnp_fault",
            format!("UPnP {action} failed: {hint} (error {error_code})."),
        )
    }
}

impl<T: IgdTransport> PortMapper for UpnpGateway<T> {
    fn external_ip(&self) -> Result<IpAddr, RemotePlayError> {
        let body = get_external_ip_body(&self.device.service_type);
        let response = self.call("GetExternalIPAddress", &body)?;
        if let Some(code) = parse_fault_code(&response) {
            return Err(Self::fault(code, "GetExternalIPAddress"));
        }
        parse_external_ip(&response).ok_or_else(|| {
            RemotePlayError::new(
                "upnp_unavailable",
                "The router did not report a WAN address; its UPnP service may be incomplete.",
            )
        })
    }

    fn add(&self, mapping: &PortMapping) -> Result<(), RemotePlayError> {
        let body = add_port_mapping_body(&self.device.service_type, mapping);
        let response = self.call("AddPortMapping", &body)?;
        if let Some(code) = parse_fault_code(&response) {
            return Err(Self::fault(code, "AddPortMapping"));
        }
        Ok(())
    }

    fn delete(&self, protocol: PortProtocol, port: u16) -> Result<(), RemotePlayError> {
        let body = delete_port_mapping_body(&self.device.service_type, protocol, port);
        let response = self.call("DeletePortMapping", &body)?;
        if let Some(code) = parse_fault_code(&response) {
            // 714: already gone, which is the desired end state.
            if code == 714 {
                return Ok(());
            }
            return Err(Self::fault(code, "DeletePortMapping"));
        }
        Ok(())
    }

    fn get(
        &self,
        protocol: PortProtocol,
        port: u16,
    ) -> Result<Option<RemoteMapping>, RemotePlayError> {
        let body = get_specific_port_mapping_body(&self.device.service_type, protocol, port);
        let response = self
            .transport
            .post(
                &self.device.control_url,
                &self.action_header("GetSpecificPortMappingEntry"),
                &body,
            )
            .map_err(|message| RemotePlayError::new("upnp_unavailable", message))?;
        if let Some(code) = parse_fault_code(&response) {
            if code == 714 {
                return Ok(None);
            }
            return Err(Self::fault(code, "GetSpecificPortMappingEntry"));
        }
        Ok(parse_mapping_entry(&response))
    }
}

/* --------------------------- address classification ----------------------- */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WanKind {
    Public,
    Cgnat,
    Private,
    Unspecified,
}

pub fn is_cgnat_ipv4(addr: Ipv4Addr) -> bool {
    let octets = addr.octets();
    // 100.64.0.0/10
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

pub fn classify_wan(addr: IpAddr) -> WanKind {
    match addr {
        IpAddr::V4(v4) if v4.is_unspecified() => WanKind::Unspecified,
        IpAddr::V4(v4) if is_cgnat_ipv4(v4) => WanKind::Cgnat,
        IpAddr::V4(v4) if is_private_ipv4(v4) => WanKind::Private,
        IpAddr::V6(v6) if v6.is_unspecified() => WanKind::Unspecified,
        IpAddr::V6(v6) if is_private_ipv6(v6) || v6.is_loopback() => WanKind::Private,
        _ => WanKind::Public,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectAssessment {
    pub allowed: bool,
    pub code: String,
    pub message: String,
    pub vpn_recommended: bool,
}

impl DirectAssessment {
    fn refused(code: &str, message: impl Into<String>) -> Self {
        Self {
            allowed: false,
            code: code.to_string(),
            message: message.into(),
            vpn_recommended: true,
        }
    }

    fn allowed(message: impl Into<String>) -> Self {
        Self {
            allowed: true,
            code: "ok".to_string(),
            message: message.into(),
            vpn_recommended: false,
        }
    }
}

/// Decide whether direct (UPnP) mode can work. CGNAT and double NAT are named
/// explicitly: a closed port alone is never used to guess CGNAT.
pub fn assess_direct_mode(
    igd_present: bool,
    wan: Option<IpAddr>,
    public_ip: Option<IpAddr>,
) -> DirectAssessment {
    if !igd_present {
        return DirectAssessment::refused(
            "upnp_unavailable",
            "Ludex found no UPnP gateway on this network. Direct mode cannot forward ports; a VPN or a manual port forward is required.",
        );
    }
    let Some(wan) = wan else {
        return DirectAssessment::refused(
            "upnp_unavailable",
            "The router's UPnP service did not report a WAN address, so Ludex cannot confirm that port forwarding would work.",
        );
    };
    match classify_wan(wan) {
        WanKind::Cgnat => {
            return DirectAssessment::refused(
                "cgnat",
                "Your ISP puts this connection behind carrier-grade NAT (the router's WAN address is in 100.64.0.0/10). Port forwarding cannot work from the internet; use a VPN or ask the ISP for a public IPv4 address.",
            )
        }
        WanKind::Private => {
            return DirectAssessment::refused(
                "double_nat",
                "The router's WAN address is a private address, so another router is doing NAT in front of it (double NAT). Forward the ports on the outer router too, or use a VPN.",
            )
        }
        WanKind::Unspecified => {
            return DirectAssessment::refused(
                "upnp_unavailable",
                "The router reported no usable WAN address. Direct mode cannot be verified.",
            )
        }
        WanKind::Public => {}
    }
    if let Some(public) = public_ip {
        if public != wan {
            return DirectAssessment::refused(
                "double_nat",
                format!(
                    "The router reports {wan} as its WAN address but the internet sees {public}. Another NAT layer (double NAT/CGNAT) or an active VPN exit node is in front of the router, so direct mode would fail. Use a VPN."
                ),
            );
        }
    }
    DirectAssessment::allowed(format!(
        "Direct mode can forward the Moonlight ports through {wan}."
    ))
}

/* ------------------------------ sunshine --------------------------------- */

pub fn parse_sunshine_conf(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        out.insert(key, value.trim().to_string());
    }
    out
}

/// Sunshine's own defaults are safe; only an explicit unsafe value is a
/// blocker. Ludex never rewrites this file.
pub fn sunshine_readiness(conf: &BTreeMap<String, String>) -> (Vec<String>, Vec<String>) {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    let web_ui = conf
        .get("origin_web_ui_allowed")
        .map(|v| v.to_ascii_lowercase());
    if web_ui.as_deref() == Some("wan") {
        blockers.push(
            "Sunshine allows its Web UI from the internet (origin_web_ui_allowed = wan). Set it back to lan before forwarding ports.".to_string(),
        );
    }
    let pin = conf
        .get("origin_pin_allowed")
        .map(|v| v.to_ascii_lowercase());
    if pin.as_deref() == Some("wan") {
        blockers.push(
            "Sunshine accepts pairing requests from the internet (origin_pin_allowed = wan). Set it to pc before forwarding ports.".to_string(),
        );
    }
    if conf.get("wan_encryption_mode").map(String::as_str) == Some("0") {
        blockers.push(
            "Sunshine's WAN encryption is disabled (wan_encryption_mode = 0). Enable it before forwarding ports.".to_string(),
        );
    }
    if conf
        .get("upnp")
        .map(|v| v.eq_ignore_ascii_case("on") || v == "true")
        == Some(true)
    {
        warnings.push(
            "Sunshine already manages its own UPnP mapping (upnp = on); Ludex will not add a second mapping.".to_string(),
        );
    }
    if web_ui.is_none() && pin.is_none() {
        warnings.push(
            "Sunshine uses its safe defaults (Web UI on the LAN, pairing on the PC).".to_string(),
        );
    }
    (warnings, blockers)
}

/// Sunshine's port base (the HTTP port) from `sunshine.conf`.
pub fn sunshine_port_base(conf: &BTreeMap<String, String>) -> u16 {
    conf.get("port")
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 5)
        .unwrap_or(47989)
}

/// HTTPS, HTTP and Web UI ports derived from the base port.
pub fn sunshine_web_ports(base: u16) -> (u16, u16, u16) {
    (base.saturating_sub(5), base, base.saturating_add(1))
}

pub fn probe_tcp(host: &str, port: u16, timeout: Duration) -> bool {
    let Ok(addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, timeout).is_ok())
}

/* -------------------------------- detection ------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunshineRemoteStatus {
    pub running: bool,
    pub config_path: Option<String>,
    pub port_base: u16,
    pub ports_open: Vec<u16>,
    pub upnp_enabled: bool,
    pub origin_web_ui_allowed: Option<String>,
    pub origin_pin_allowed: Option<String>,
    pub wan_encryption_mode: Option<u32>,
    pub blockers: Vec<String>,
    pub warnings: Vec<String>,
    pub message: String,
}

pub fn sunshine_remote_status(
    settings: &LocalSettings,
    process_names: &[String],
    probe: &dyn Fn(&str, u16) -> bool,
) -> SunshineRemoteStatus {
    let status = streaming::sunshine_status(settings);
    let config_path = status
        .conf_path
        .clone()
        .filter(|path| Path::new(path).is_file());
    let conf = config_path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|text| parse_sunshine_conf(&text))
        .unwrap_or_default();
    let (warnings, blockers) = sunshine_readiness(&conf);
    let port_base = sunshine_port_base(&conf);
    let (https, http, web) = sunshine_web_ports(port_base);
    let candidates = [https, http, web, 48010];
    let mut ports_open = Vec::new();
    for port in candidates {
        if probe("127.0.0.1", port) && !ports_open.contains(&port) {
            ports_open.push(port);
        }
    }
    let process_running = process_names.iter().any(|name| name.contains("sunshine"));
    let running = process_running || !ports_open.is_empty();
    let message = if running {
        format!(
            "Sunshine is running and listening on {} port(s).",
            ports_open.len()
        )
    } else {
        "Sunshine does not appear to be running. Start it before testing a Moonlight connection."
            .to_string()
    };
    SunshineRemoteStatus {
        running,
        config_path,
        port_base,
        ports_open,
        upnp_enabled: conf
            .get("upnp")
            .map(|value| value.eq_ignore_ascii_case("on") || value == "true")
            .unwrap_or(false),
        origin_web_ui_allowed: conf.get("origin_web_ui_allowed").cloned(),
        origin_pin_allowed: conf.get("origin_pin_allowed").cloned(),
        wan_encryption_mode: conf
            .get("wan_encryption_mode")
            .and_then(|value| value.parse().ok()),
        blockers,
        warnings,
        message,
    }
}

/* --------------------------- network inventory ---------------------------- */

/// One address reported by the OS for a local interface, before classification
/// (#206). Kept separate from [`LocalAddress`] so classification and selection
/// stay pure and can be unit-tested with fixtures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAddress {
    pub interface: String,
    pub address: IpAddr,
    pub prefix: u8,
}

/// What an address is useful for when Moonlight connects from another network.
/// Only `vpn`, `lan` and `ipv6` are ever advertised (#206).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressKind {
    Vpn,
    Lan,
    Ipv6,
    Cgnat,
    Public,
    Ula,
    LinkLocal,
    Loopback,
    Virtual,
}

/// A classified local address (#206). `service` is only set for a known
/// VPN/mesh adapter: a CGNAT or public address on an unknown interface is never
/// labelled as a VPN.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAddress {
    pub address: String,
    pub interface: String,
    pub kind: AddressKind,
    pub service: Option<String>,
    pub prefix: u8,
}

/// Every interface name and classified address the OS reports (#206).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkInventory {
    pub interfaces: Vec<String>,
    pub addresses: Vec<LocalAddress>,
}

/// Known VPN/mesh adapter names (#206). sysinfo exposes the adapter name on
/// every platform, so it is the only signal that also works for a tunnel which
/// is not the default route.
pub fn vpn_interface_service(interface: &str) -> Option<&'static str> {
    let lower = interface.trim().to_ascii_lowercase();
    let stem = lower.trim_end_matches(|value: char| value.is_ascii_digit());
    if lower == "wt0" || lower.starts_with("netbird") {
        return Some("Netbird");
    }
    if lower.starts_with("tailscale") {
        return Some("Tailscale");
    }
    if lower.starts_with("headscale") {
        return Some("Headscale");
    }
    if lower.starts_with("zerotier") || (lower.starts_with("zt") && lower.len() > 2) {
        return Some("ZeroTier");
    }
    if lower.starts_with("wireguard") || (stem == "wg" && lower != "wg") {
        return Some("WireGuard");
    }
    if lower.starts_with("openvpn")
        || lower.contains("tap-windows")
        || lower.contains("wintun")
        || ((stem == "tun" || stem == "tap" || stem == "utun") && lower != stem)
    {
        return Some("Tunnel");
    }
    None
}

/// Bridge, container and hypervisor adapters never reach another network, so
/// they are listed for transparency but never selected (#206).
pub fn is_virtual_interface(interface: &str) -> bool {
    const PREFIXES: [&str; 15] = [
        "docker",
        "br-",
        "veth",
        "virbr",
        "vmnet",
        "vboxnet",
        "vbox",
        "lxc",
        "lxd",
        "cni",
        "flannel",
        "podman",
        "wsl",
        "vethernet",
        "virtualbox",
    ];
    const CONTAINS: [&str; 4] = ["vmware", "hyper-v", "virtual adapter", "host-only"];
    let lower = interface.trim().to_ascii_lowercase();
    lower == "lo"
        || lower.starts_with("loopback")
        || PREFIXES.iter().any(|prefix| lower.starts_with(prefix))
        || CONTAINS.iter().any(|needle| lower.contains(needle))
        || (lower.starts_with("bridge") && lower != "bridge")
}

pub fn classify_interface_address(
    interface: &str,
    address: IpAddr,
) -> (AddressKind, Option<String>) {
    let service = vpn_interface_service(interface);
    match address {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                (AddressKind::Loopback, None)
            } else if v4.is_link_local() {
                (AddressKind::LinkLocal, None)
            } else if let Some(service) = service {
                (AddressKind::Vpn, Some(service.to_string()))
            } else if is_cgnat_ipv4(v4) {
                // 100.64.0.0/10 is also the Tailscale/Netbird range, but only
                // an adapter that names the service is treated as a VPN. An
                // ISP CGNAT address must never be advertised (#206).
                (AddressKind::Cgnat, None)
            } else if v4.is_private() {
                if is_virtual_interface(interface) {
                    (AddressKind::Virtual, None)
                } else {
                    (AddressKind::Lan, None)
                }
            } else {
                (AddressKind::Public, None)
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                (AddressKind::Loopback, None)
            } else if is_link_local_ipv6(v6) {
                (AddressKind::LinkLocal, None)
            } else if let Some(service) = service {
                (AddressKind::Vpn, Some(service.to_string()))
            } else if is_ula_ipv6(v6) {
                (AddressKind::Ula, None)
            } else if is_global_ipv6(v6) {
                (AddressKind::Ipv6, None)
            } else {
                (AddressKind::Public, None)
            }
        }
    }
}

/// Classifies and de-duplicates an interface snapshot, ordered from the most to
/// the least useful for remote play (#206).
pub fn classify_addresses(interfaces: &[InterfaceAddress]) -> Vec<LocalAddress> {
    let mut seen = BTreeSet::new();
    let mut classified = Vec::new();
    for entry in interfaces {
        if !seen.insert((entry.interface.clone(), entry.address)) {
            continue;
        }
        let (kind, service) = classify_interface_address(&entry.interface, entry.address);
        classified.push(LocalAddress {
            address: entry.address.to_string(),
            interface: entry.interface.clone(),
            kind,
            service,
            prefix: entry.prefix,
        });
    }
    classified.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| left.address.cmp(&right.address))
    });
    classified
}

/// Reads every interface and address from the OS (#206). No shelling out: the
/// same sysinfo snapshot also lists adapters that are down or have no address,
/// which is what lets the wizard explain a missing VPN address.
pub fn local_network() -> NetworkInventory {
    let Ok(networks) = sysinfo::Networks::new_with_refreshed_list() else {
        return NetworkInventory::default();
    };
    let mut inventory = NetworkInventory::default();
    let mut raw = Vec::new();
    for (name, data) in &networks {
        let name = name.to_string_lossy().to_string();
        inventory.interfaces.push(name.clone());
        for network in data.ip_networks() {
            raw.push(InterfaceAddress {
                interface: name.clone(),
                address: network.addr,
                prefix: network.prefix,
            });
        }
    }
    inventory.interfaces.sort();
    inventory.interfaces.dedup();
    inventory.addresses = classify_addresses(&raw);
    inventory
}

/* --------------------------------- VPN ------------------------------------ */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnAddress {
    pub address: String,
    pub interface: String,
    pub service: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnStatus {
    pub detected: bool,
    pub services: Vec<String>,
    /// Address the user enters in Moonlight when the VPN is up (#206).
    pub address: Option<String>,
    /// Every VPN/mesh address, so several active VPNs can be offered (#206).
    pub addresses: Vec<VpnAddress>,
    /// VPN adapters that carry no usable address, named so the wizard can
    /// explain the LAN fallback instead of hiding it (#206).
    pub interfaces: Vec<String>,
    pub message: String,
}

pub fn vpn_services(process_names: &[String]) -> Vec<String> {
    const KNOWN: [(&str, &str); 7] = [
        ("tailscaled", "Tailscale"),
        ("tailscale", "Tailscale"),
        ("wireguard", "WireGuard"),
        ("zerotier-one", "ZeroTier"),
        ("zerotier", "ZeroTier"),
        ("netbird", "Netbird"),
        ("headscale", "Headscale"),
    ];
    let mut found: Vec<String> = Vec::new();
    for name in process_names {
        let lower = name.to_ascii_lowercase();
        for (needle, label) in KNOWN {
            if lower.contains(needle) && !found.iter().any(|value| value == label) {
                found.push(label.to_string());
            }
        }
    }
    found
}

/// Mesh networks are the most reliable remote-play path, so their address is
/// preferred when several VPNs are up. The order is stable for tests.
fn service_rank(service: &str) -> u8 {
    match service {
        "Tailscale" => 0,
        "Netbird" => 1,
        "Headscale" => 2,
        "ZeroTier" => 3,
        "WireGuard" => 4,
        _ => 5,
    }
}

pub fn vpn_status(
    process_names: &[String],
    addresses: &[LocalAddress],
    interfaces: &[String],
) -> VpnStatus {
    let mut services = vpn_services(process_names);
    let mut vpn_addresses: Vec<VpnAddress> = addresses
        .iter()
        .filter(|entry| entry.kind == AddressKind::Vpn)
        .filter_map(|entry| {
            entry.service.as_ref().map(|service| VpnAddress {
                address: entry.address.clone(),
                interface: entry.interface.clone(),
                service: service.clone(),
            })
        })
        .collect();
    vpn_addresses.sort_by(|left, right| {
        service_rank(&left.service)
            .cmp(&service_rank(&right.service))
            .then_with(|| left.interface.cmp(&right.interface))
            .then_with(|| left.address.cmp(&right.address))
    });
    // A tunnel can be up without its process being visible (permissions, a
    // service account); the adapter name still names the service.
    for service in vpn_addresses
        .iter()
        .map(|entry| entry.service.clone())
        .chain(
            interfaces
                .iter()
                .filter_map(|name| vpn_interface_service(name).map(str::to_string)),
        )
    {
        if !services.iter().any(|value| value == &service) {
            services.push(service);
        }
    }
    let mut vpn_interfaces: Vec<String> = interfaces
        .iter()
        .filter(|name| vpn_interface_service(name).is_some())
        .filter(|name| !vpn_addresses.iter().any(|entry| &entry.interface == *name))
        .cloned()
        .collect();
    vpn_interfaces.sort();
    vpn_interfaces.dedup();
    let detected = !services.is_empty() || !vpn_addresses.is_empty() || !vpn_interfaces.is_empty();
    let address = vpn_addresses.first().map(|entry| entry.address.clone());
    let message = if !detected {
        "No VPN service was detected. A VPN (Tailscale, WireGuard, ZeroTier, Netbird or Headscale) is the reliable way to play away from home without exposing ports."
            .to_string()
    } else if let Some(entry) = vpn_addresses.first() {
        format!(
            "{} detected. Enter {} ({}) in Moonlight from the other network; no port mapping is needed.",
            services.join(", "),
            entry.address,
            entry.service
        )
    } else {
        let where_ = if vpn_interfaces.is_empty() {
            String::new()
        } else {
            format!(" on {}", vpn_interfaces.join(", "))
        };
        format!(
            "{}{where_} detected, but no VPN address is assigned to this machine. The LAN address works at home only; bring the tunnel up or copy the address from the VPN service.",
            services.join(", ")
        )
    };
    VpnStatus {
        detected,
        services,
        address,
        addresses: vpn_addresses,
        interfaces: vpn_interfaces,
        message,
    }
}

/// The address the wizard advertises, with the reason it was chosen (#206).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertisedAddress {
    pub address: Option<String>,
    /// `vpn`, `lan`, `ipv6` or `none`.
    pub source: String,
    pub interface: Option<String>,
    pub service: Option<String>,
    pub reason: String,
}

fn select_lan<'a>(
    addresses: &'a [LocalAddress],
    route: Option<Ipv4Addr>,
) -> Option<&'a LocalAddress> {
    let route = route.map(|addr| addr.to_string());
    addresses
        .iter()
        .filter(|entry| entry.kind == AddressKind::Lan)
        .find(|entry| route.as_deref() == Some(entry.address.as_str()))
        .or_else(|| {
            addresses
                .iter()
                .find(|entry| entry.kind == AddressKind::Lan)
        })
}

/// VPN first, then the LAN address (preferring the default route), then a
/// global IPv6 address. Never a Docker bridge, a CGNAT WAN address or
/// loopback (#206).
pub fn select_advertised(
    addresses: &[LocalAddress],
    vpn: &VpnStatus,
    route: Option<Ipv4Addr>,
) -> AdvertisedAddress {
    if let Some(address) = &vpn.address {
        let entry = addresses
            .iter()
            .find(|entry| entry.kind == AddressKind::Vpn && &entry.address == address);
        let interface = entry.map(|entry| entry.interface.clone());
        let service = entry.and_then(|entry| entry.service.clone());
        let label = service.as_deref().unwrap_or("VPN");
        let reason = match &interface {
            Some(interface) => format!("{label} address {address} on {interface}"),
            None => format!("{label} address {address}"),
        };
        return AdvertisedAddress {
            address: Some(address.clone()),
            source: "vpn".to_string(),
            interface,
            service,
            reason,
        };
    }
    if let Some(entry) = select_lan(addresses, route) {
        let reason = if vpn.detected {
            format!(
                "{} detected, but no VPN address is assigned; showing the LAN address {}, which only works at home",
                vpn.services.join(", "),
                entry.address
            )
        } else {
            format!(
                "LAN address {} on {}; no VPN was detected, so this only works at home",
                entry.address, entry.interface
            )
        };
        return AdvertisedAddress {
            address: Some(entry.address.clone()),
            source: "lan".to_string(),
            interface: Some(entry.interface.clone()),
            service: None,
            reason,
        };
    }
    if let Some(entry) = addresses
        .iter()
        .find(|entry| entry.kind == AddressKind::Ipv6)
    {
        return AdvertisedAddress {
            address: Some(entry.address.clone()),
            source: "ipv6".to_string(),
            interface: Some(entry.interface.clone()),
            service: None,
            reason: format!(
                "Global IPv6 address {} on {}; no VPN or LAN address was found",
                entry.address, entry.interface
            ),
        };
    }
    AdvertisedAddress {
        address: None,
        source: "none".to_string(),
        interface: None,
        service: None,
        reason: "No usable local address was found. Connect to a network and detect again."
            .to_string(),
    }
}

pub fn local_ipv4() -> Option<Ipv4Addr> {
    // Connecting a UDP socket sends no packet; it only asks the routing table
    // which local address would reach the internet.
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(addr) if !addr.is_loopback() && !addr.is_unspecified() => Some(addr),
        _ => None,
    }
}

/// True for a globally routable IPv6 address (2000::/3). Link-local and
/// unique-local addresses are not reachable from another network.
pub fn is_global_ipv6(addr: Ipv6Addr) -> bool {
    let segments = addr.segments();
    (segments[0] & 0xe000) == 0x2000
}

pub fn local_ipv6() -> Option<Ipv6Addr> {
    let socket = UdpSocket::bind("[::]:0").ok()?;
    socket.connect("[2001:4860:4860::8888]:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V6(addr) if is_global_ipv6(addr) => Some(addr),
        _ => None,
    }
}

pub fn running_process_names() -> Vec<String> {
    let Ok(mut system) = sysinfo::System::new() else {
        return Vec::new();
    };
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    system
        .processes()
        .values()
        .map(|process| process.name().to_string_lossy().to_ascii_lowercase())
        .collect()
}

/* ----------------------------- verification ------------------------------- */

pub trait Reachability {
    /// Best-effort TCP handshake from this machine. `false` is not proof that
    /// the port is closed, because many routers refuse NAT loopback.
    fn tcp(&self, address: SocketAddr) -> bool;
}

pub struct TcpProbe {
    pub timeout: Duration,
}

impl Reachability for TcpProbe {
    fn tcp(&self, address: SocketAddr) -> bool {
        TcpStream::connect_timeout(&address, self.timeout).is_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationPort {
    pub protocol: PortProtocol,
    pub port: u16,
    /// `reachable`, `mapped`, `not_mapped`, `conflict` or `unknown`.
    pub status: String,
    pub mapped: bool,
    /// `None` for UDP: without a partner node on the outside there is no
    /// protocol-aware echo, so UDP is confirmed by the IGD query only.
    pub reachable: Option<bool>,
    pub description: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationReport {
    /// Every planned port is mapped to this machine.
    pub verified: bool,
    pub public_ip: Option<String>,
    pub lan_ip: Option<String>,
    pub reachable: Option<bool>,
    pub ports: Vec<VerificationPort>,
    pub notes: Vec<String>,
    pub message: String,
}

pub fn audit_mappings(
    mapper: &dyn PortMapper,
    plan: &PortPlan,
    active: &[ActiveMapping],
    public_ip: Option<IpAddr>,
    lan_ip: Option<&str>,
    probe: &dyn Reachability,
) -> VerificationReport {
    let mut ports = Vec::with_capacity(plan.total);
    let mut notes = Vec::new();
    let mut all_mapped = true;
    let mut tcp_attempted = 0usize;
    let mut tcp_reachable = 0usize;

    for mapping in plan_mappings(plan, lan_ip.unwrap_or("0.0.0.0")) {
        let stored = active.iter().find(|item| {
            item.protocol == mapping.protocol && item.external_port == mapping.external_port
        });
        let queried = match mapper.get(mapping.protocol, mapping.external_port) {
            Ok(value) => value,
            Err(error) => {
                all_mapped = false;
                ports.push(VerificationPort {
                    protocol: mapping.protocol,
                    port: mapping.external_port,
                    status: "unknown".to_string(),
                    mapped: false,
                    reachable: None,
                    description: stored.map(|item| item.description.clone()),
                    note: error.message,
                });
                continue;
            }
        };
        let Some(entry) = queried else {
            all_mapped = false;
            ports.push(VerificationPort {
                protocol: mapping.protocol,
                port: mapping.external_port,
                status: "not_mapped".to_string(),
                mapped: false,
                reachable: None,
                description: stored.map(|item| item.description.clone()),
                note: "The router has no mapping for this port; the lease may have expired."
                    .to_string(),
            });
            continue;
        };
        if !is_ludex_mapping(&entry.description) {
            all_mapped = false;
            ports.push(VerificationPort {
                protocol: mapping.protocol,
                port: mapping.external_port,
                status: "conflict".to_string(),
                mapped: false,
                reachable: None,
                description: Some(entry.description.clone()),
                note: format!(
                    "This port is mapped to {} by another program. Ludex did not overwrite it.",
                    entry.internal_client
                ),
            });
            continue;
        }
        let external = mapping.external_port;
        let mut reachable = None;
        let (status, note) = match (mapping.protocol, public_ip) {
            (PortProtocol::Tcp, Some(public_ip)) => {
                let address = SocketAddr::new(public_ip, external);
                let ok = probe.tcp(address);
                tcp_attempted += 1;
                if ok {
                    tcp_reachable += 1;
                }
                reachable = Some(ok);
                if ok {
                    (
                        "reachable".to_string(),
                        format!("The router accepted a TCP connection to {public_ip}:{external}."),
                    )
                } else {
                    (
                        "mapped".to_string(),
                        "The router reports the mapping, but this machine could not reach it from the LAN. Many routers block NAT loopback; test from mobile data before changing anything.".to_string(),
                    )
                }
            }
            (PortProtocol::Tcp, None) => (
                "mapped".to_string(),
                "The mapping exists, but no public address was available for a reachability probe."
                    .to_string(),
            ),
            (PortProtocol::Udp, _) => (
                "mapped".to_string(),
                "The mapping exists. UDP reachability needs a partner node outside the LAN, so this is confirmed by the router query only.".to_string(),
            ),
        };
        ports.push(VerificationPort {
            protocol: mapping.protocol,
            port: mapping.external_port,
            status,
            mapped: true,
            reachable,
            description: Some(entry.description),
            note,
        });
    }

    if tcp_attempted > 0 && tcp_reachable == 0 {
        notes.push(
            "None of the TCP ports answered from this machine. That is normal when the router does not support NAT loopback; verify from a phone on mobile data.".to_string(),
        );
    }
    if plan.tcp.iter().any(|port| *port == 47989) {
        notes.push(
            "Only the forwarded ports are exposed. TCP 47990 (Sunshine Web UI) and UDP 5353 (mDNS) stay on the LAN.".to_string(),
        );
    }
    let reachable = if tcp_attempted == 0 {
        None
    } else {
        Some(tcp_reachable == tcp_attempted)
    };
    let message = if all_mapped {
        "Every planned port is mapped to this machine. Test it from another network with Moonlight."
            .to_string()
    } else {
        "Some planned ports are missing or owned by another program. Remove the mapping and retry before testing from outside."
            .to_string()
    };
    VerificationReport {
        verified: all_mapped,
        public_ip: public_ip.map(|addr| addr.to_string()),
        lan_ip: lan_ip.map(str::to_string),
        reachable,
        ports,
        notes,
        message,
    }
}

/* -------------------------------- apply ---------------------------------- */

#[derive(Debug)]
pub struct ApplyFailure {
    pub error: RemotePlayError,
    /// Mappings added before the failure, so the caller can roll them back
    /// instead of leaving a half-enabled host behind.
    pub applied: Vec<ActiveMapping>,
}

pub fn apply_mappings(
    mapper: &dyn PortMapper,
    mappings: &[PortMapping],
    now: i64,
) -> Result<Vec<ActiveMapping>, ApplyFailure> {
    let mut applied = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        match mapper.get(mapping.protocol, mapping.external_port) {
            Ok(Some(existing)) if is_ludex_mapping(&existing.description) => {}
            Ok(Some(existing)) => {
                return Err(ApplyFailure {
                    error: RemotePlayError::new(
                        "port_owned",
                        format!(
                            "{} {} is already mapped to {} by \"{}\". Ludex will not overwrite another program's mapping; remove it on the router first.",
                            mapping.protocol,
                            mapping.external_port,
                            existing.internal_client,
                            existing.description
                        ),
                    ),
                    applied,
                })
            }
            Ok(None) => {}
            Err(error) => {
                return Err(ApplyFailure {
                    error,
                    applied,
                })
            }
        }
        if let Err(error) = mapper.add(mapping) {
            return Err(ApplyFailure { error, applied });
        }
        applied.push(ActiveMapping::from_mapping(mapping, now));
    }
    Ok(applied)
}

/* -------------------------------- cleanup --------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupPort {
    pub protocol: PortProtocol,
    pub port: u16,
    /// `removed`, `absent` or `failed`.
    pub status: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub complete: bool,
    pub removed: usize,
    pub ports: Vec<CleanupPort>,
    pub message: String,
}

pub fn remove_mappings(mapper: &dyn PortMapper, mappings: &[ActiveMapping]) -> CleanupReport {
    let mut ports = Vec::with_capacity(mappings.len());
    let mut removed = 0usize;
    let mut failed = 0usize;
    for mapping in mappings {
        // A port the router no longer knows is already in the desired state;
        // reporting it as "absent" is more accurate than claiming a delete.
        if matches!(
            mapper.get(mapping.protocol, mapping.external_port),
            Ok(None)
        ) {
            ports.push(CleanupPort {
                protocol: mapping.protocol,
                port: mapping.external_port,
                status: "absent".to_string(),
                note: "The router had no mapping for this port.".to_string(),
            });
            continue;
        }
        let note = match mapper.delete(mapping.protocol, mapping.external_port) {
            Ok(()) => match mapper.get(mapping.protocol, mapping.external_port) {
                Ok(None) => {
                    removed += 1;
                    ports.push(CleanupPort {
                        protocol: mapping.protocol,
                        port: mapping.external_port,
                        status: "removed".to_string(),
                        note: "Removed and verified.".to_string(),
                    });
                    continue;
                }
                Ok(Some(entry)) => format!(
                    "The router still reports a mapping to {} after the delete.",
                    entry.internal_client
                ),
                Err(error) => format!(
                    "The delete was sent but verification failed: {}",
                    error.message
                ),
            },
            Err(error) => error.message,
        };
        failed += 1;
        ports.push(CleanupPort {
            protocol: mapping.protocol,
            port: mapping.external_port,
            status: "failed".to_string(),
            note,
        });
    }
    let complete = failed == 0;
    let message = if complete {
        if mappings.is_empty() {
            "No Ludex mapping was active.".to_string()
        } else {
            format!("Removed and verified {removed} port mapping(s). No Ludex port stays open.")
        }
    } else {
        format!(
            "{failed} port mapping(s) could not be removed or verified. The lease will expire at the router, or remove them from the router's UPnP page."
        )
    };
    CleanupReport {
        complete,
        removed,
        ports,
        message,
    }
}

/* ------------------------------- persistence ------------------------------ */

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RemotePlaySession {
    pub gateway: Option<IgdDevice>,
    pub mappings: Vec<ActiveMapping>,
    pub public_ip: Option<String>,
    pub lan_ip: Option<String>,
    pub updated_at: Option<String>,
}

impl RemotePlaySession {
    pub fn is_active(&self) -> bool {
        !self.mappings.is_empty()
    }
}

pub fn session_path(app_data: &Path) -> PathBuf {
    app_data.join("remote_play").join("session.json")
}

pub fn load_session(path: &Path) -> RemotePlaySession {
    let Ok(text) = std::fs::read_to_string(path) else {
        return RemotePlaySession::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_session(path: &Path, session: &RemotePlaySession) -> Result<(), RemotePlayError> {
    let Some(parent) = path.parent() else {
        return Err(RemotePlayError::new(
            "state_unavailable",
            "The remote play state folder is unavailable.",
        ));
    };
    std::fs::create_dir_all(parent).map_err(|error| {
        RemotePlayError::new(
            "state_unavailable",
            format!("Could not create {}: {error}", parent.display()),
        )
    })?;
    let temporary = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(session).map_err(|error| {
        RemotePlayError::new(
            "state_unavailable",
            format!("Could not encode the remote play state: {error}"),
        )
    })?;
    std::fs::write(&temporary, payload).map_err(|error| {
        RemotePlayError::new(
            "state_unavailable",
            format!("Could not write {}: {error}", temporary.display()),
        )
    })?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        RemotePlayError::new(
            "state_unavailable",
            format!("Could not save {}: {error}", path.display()),
        )
    })
}

pub fn clear_session(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Mappings whose remaining lease is under half of the requested lease. The
/// wizard refreshes those before the router drops them.
pub fn leases_due(session: &RemotePlaySession, now: i64, lease_secs: u64) -> Vec<PortMapping> {
    let threshold = (lease_secs / 2) as i64;
    session
        .mappings
        .iter()
        .filter(|mapping| mapping.expires_at.saturating_sub(now) < threshold)
        .map(ActiveMapping::to_mapping)
        .collect()
}

/// Shared runtime state managed by Tauri. The file is the source of truth so a
/// restarted app can still delete what the previous run mapped.
pub struct RemotePlayState {
    pub path: PathBuf,
    inner: Arc<Mutex<RemotePlaySession>>,
}

impl RemotePlayState {
    pub fn new(app_data: &Path) -> Self {
        let path = session_path(app_data);
        let session = load_session(&path);
        Self {
            path,
            inner: Arc::new(Mutex::new(session)),
        }
    }

    pub fn snapshot(&self) -> RemotePlaySession {
        match self.inner.lock() {
            Ok(session) => session.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Handle for a blocking worker: the session mutex and the file path.
    pub fn shared(&self) -> (PathBuf, Arc<Mutex<RemotePlaySession>>) {
        (self.path.clone(), Arc::clone(&self.inner))
    }
}

/// Remove leftover mappings from a previous run. Called once at startup on a
/// background thread; a failed cleanup leaves the file in place for the next
/// run and the router's lease expiry still bounds the exposure.
pub fn spawn_startup_cleanup(app_data: &Path) {
    let path = session_path(app_data);
    if !path.is_file() {
        return;
    }
    std::thread::spawn(move || {
        let session = load_session(&path);
        let Some(device) = session.gateway.clone() else {
            clear_session(&path);
            return;
        };
        if !session.is_active() {
            clear_session(&path);
            return;
        }
        let mapper = UpnpGateway::new(device, HttpTransport::new(Duration::from_secs(3)));
        let report = remove_mappings(&mapper, &session.mappings);
        if report.complete {
            clear_session(&path);
            tracing::info!(
                removed = report.removed,
                "removed leftover remote play port mappings"
            );
        } else if report.removed > 0 {
            let remaining: Vec<ActiveMapping> = session
                .mappings
                .iter()
                .filter(|mapping| {
                    report.ports.iter().any(|port| {
                        port.port == mapping.external_port
                            && port.protocol == mapping.protocol
                            && port.status == "failed"
                    })
                })
                .cloned()
                .collect();
            let mut next = session;
            next.mappings = remaining;
            if save_session(&path, &next).is_err() {
                tracing::warn!("could not persist the remote play cleanup result");
            }
            tracing::warn!(
                failed = report
                    .ports
                    .iter()
                    .filter(|port| port.status == "failed")
                    .count(),
                "some leftover remote play mappings could not be removed"
            );
        } else {
            tracing::warn!("could not clean up leftover remote play mappings");
        }
    });
}

/* ------------------------------- status ---------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpnpStatus {
    pub available: bool,
    pub friendly_name: Option<String>,
    pub model: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlayStatus {
    pub platform: String,
    pub lan_ip: Option<String>,
    pub ipv6: Option<String>,
    pub public_ip: Option<String>,
    pub vpn: VpnStatus,
    /// Every local address, classified (#206).
    pub addresses: Vec<LocalAddress>,
    /// The address the wizard shows, with the reason it was chosen (#206).
    pub advertised: AdvertisedAddress,
    pub sunshine: SunshineRemoteStatus,
    pub upnp: UpnpStatus,
    pub direct: DirectAssessment,
    pub active: RemotePlaySession,
    pub lease_default_secs: u64,
    pub lease_min_secs: u64,
    pub lease_max_secs: u64,
    pub warnings: Vec<String>,
    pub message: String,
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

fn probe_timeout() -> Duration {
    Duration::from_millis(350)
}

fn discovery_timeout() -> Duration {
    Duration::from_millis(2200)
}

/// Capability detection used by the wizard's first step.
pub fn detect(
    settings: &LocalSettings,
    process_names: &[String],
    igd: Option<IgdDevice>,
    igd_error: Option<RemotePlayError>,
    public_ip: Option<IpAddr>,
    active: RemotePlaySession,
) -> RemotePlayStatus {
    let inventory = local_network();
    let route = local_ipv4();
    let vpn = vpn_status(process_names, &inventory.addresses, &inventory.interfaces);
    let advertised = select_advertised(&inventory.addresses, &vpn, route);
    let lan = select_lan(&inventory.addresses, route)
        .map(|entry| entry.address.clone())
        .or_else(|| {
            route
                .filter(|addr| is_private_ipv4(*addr) && !is_cgnat_ipv4(*addr))
                .map(|addr| addr.to_string())
        });
    let ipv6 = inventory
        .addresses
        .iter()
        .find(|entry| entry.kind == AddressKind::Ipv6)
        .map(|entry| entry.address.clone())
        .or_else(|| local_ipv6().map(|addr| addr.to_string()));
    tracing::debug!(
        address = advertised.address.as_deref().unwrap_or("none"),
        interface = advertised.interface.as_deref().unwrap_or("none"),
        source = advertised.source.as_str(),
        "remote play advertised address"
    );
    let sunshine = sunshine_remote_status(settings, process_names, &|host, port| {
        probe_tcp(host, port, probe_timeout())
    });

    let mut warnings = Vec::new();
    let igd_error_code = igd_error.as_ref().map(|error| error.code.clone());
    let (upnp, wan) = match igd {
        Some(device) => {
            let mapper =
                UpnpGateway::new(device.clone(), HttpTransport::new(Duration::from_secs(4)));
            let wan = match mapper.external_ip() {
                Ok(address) => Some(address),
                Err(error) => {
                    warnings.push(format!(
                        "The router's WAN address could not be read: {}",
                        error.message
                    ));
                    None
                }
            };
            (
                UpnpStatus {
                    available: true,
                    friendly_name: Some(device.friendly_name.clone())
                        .filter(|name| !name.is_empty()),
                    model: Some(device.model.clone()).filter(|name| !name.is_empty()),
                    message: "A UPnP gateway is available on the LAN.".to_string(),
                },
                wan,
            )
        }
        None => (
            UpnpStatus {
                available: false,
                friendly_name: None,
                model: None,
                message: igd_error
                    .map(|error| error.message)
                    .unwrap_or_else(|| "No UPnP gateway was found on this network.".to_string()),
            },
            None,
        ),
    };
    let mut direct = assess_direct_mode(upnp.available, wan, public_ip);
    if !upnp.available && igd_error_code.as_deref() == Some("public_igd") {
        // Keep the specific refusal instead of collapsing it to "no gateway".
        direct = DirectAssessment::refused("public_igd", upnp.message.clone());
    }
    if let Some(wan) = wan {
        if is_private_ip(wan) && direct.allowed {
            warnings.push(
                "The router's WAN address is private; double NAT may still block remote play."
                    .to_string(),
            );
        }
    }
    if sunshine.upnp_enabled {
        warnings.push(
            "Sunshine has UPnP enabled. Ludex will not add a second mapping: use Sunshine's own setup or turn upnp off in sunshine.conf."
                .to_string(),
        );
    }

    let message = if active.is_active() {
        "Direct mode is active. Disable it when you finish to remove every mapping.".to_string()
    } else if vpn.detected {
        "A VPN is available: it is the recommended way to play away from home. Direct mode stays optional."
            .to_string()
    } else {
        direct.message.clone()
    };

    RemotePlayStatus {
        platform: platform_name().to_string(),
        lan_ip: lan,
        ipv6,
        public_ip: public_ip.map(|addr| addr.to_string()),
        vpn,
        addresses: inventory.addresses,
        advertised,
        sunshine,
        upnp,
        direct,
        active,
        lease_default_secs: DEFAULT_LEASE_SECS,
        lease_min_secs: MIN_LEASE_SECS,
        lease_max_secs: MAX_LEASE_SECS,
        warnings,
        message,
    }
}

/* -------------------------------- reports --------------------------------- */

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemotePlayReport {
    pub enabled: bool,
    pub public_ip: Option<String>,
    pub lan_ip: Option<String>,
    pub lease_secs: u64,
    pub mappings: Vec<ActiveMapping>,
    pub verification: VerificationReport,
    pub message: String,
}

/* -------------------------------- commands -------------------------------- */

async fn fetch_public_ip(http: &reqwest::Client) -> Option<IpAddr> {
    for url in ["https://api.ipify.org", "https://ifconfig.me/ip"] {
        let Ok(response) = http
            .get(url)
            .timeout(Duration::from_secs(4))
            .header("User-Agent", "ludex-remote-play/1")
            .send()
            .await
        else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(text) = response.text().await else {
            continue;
        };
        if let Ok(address) = text.trim().parse::<IpAddr>() {
            if !is_private_ip(address) {
                return Some(address);
            }
        }
    }
    None
}

#[tauri::command]
pub async fn remote_play_status(
    app: tauri::AppHandle,
    probe_public: Option<bool>,
) -> Result<RemotePlayStatus, RemotePlayError> {
    use tauri::Manager;
    let state = app.state::<crate::state::AppState>();
    let settings = state.settings_snapshot();
    let http = state.http.clone();
    let active = app.state::<RemotePlayState>().snapshot();
    // The external address probe is opt-in: the Settings summary never
    // contacts a third party, only the wizard's detection step does.
    let public_ip = if probe_public.unwrap_or(false) {
        fetch_public_ip(&http).await
    } else {
        None
    };
    tauri::async_runtime::spawn_blocking(move || {
        let process_names = running_process_names();
        let transport = HttpTransport::new(Duration::from_secs(3));
        let (igd, igd_error) = match discover_igd(&transport, discovery_timeout()) {
            Ok(device) => (Some(device), None),
            Err(error) => (None, Some(error)),
        };
        detect(&settings, &process_names, igd, igd_error, public_ip, active)
    })
    .await
    .map_err(|error| {
        RemotePlayError::new(
            "detection_failed",
            format!("The network detection task failed: {error}"),
        )
    })
}

/// The exact consent plan. Pure and cheap: the UI shows it before enabling.
#[tauri::command]
pub fn remote_play_plan(microphone: Option<bool>, lease_secs: Option<u64>) -> PortPlan {
    plan_ports(microphone.unwrap_or(false), lease_secs)
}

#[tauri::command]
pub async fn remote_play_enable(
    app: tauri::AppHandle,
    microphone: Option<bool>,
    lease_secs: Option<u64>,
    confirm: bool,
) -> Result<RemotePlayReport, RemotePlayError> {
    use tauri::Manager;
    if !confirm {
        return Err(RemotePlayError::new(
            "consent_required",
            "Port mapping needs explicit consent: review the ports, protocol and lease first.",
        ));
    }
    let plan = plan_ports(microphone.unwrap_or(false), lease_secs);
    validate_plan(&plan)?;
    let state = app.state::<crate::state::AppState>();
    let settings = state.settings_snapshot();
    let http = state.http.clone();
    let remote = app.state::<RemotePlayState>();
    let (path, shared) = remote.shared();
    let public_ip = fetch_public_ip(&http).await;

    tauri::async_runtime::spawn_blocking(move || {
        let process_names = running_process_names();
        let transport = HttpTransport::new(Duration::from_secs(4));
        let device = discover_igd(&transport, discovery_timeout())?;
        let gateway = UpnpGateway::new(device.clone(), HttpTransport::new(Duration::from_secs(4)));
        let wan = gateway.external_ip()?;
        let assessment = assess_direct_mode(true, Some(wan), public_ip);
        if !assessment.allowed {
            return Err(RemotePlayError::new(assessment.code.as_str(), assessment.message));
        }
        let sunshine = sunshine_remote_status(&settings, &process_names, &|host, port| {
            probe_tcp(host, port, probe_timeout())
        });
        if let Some(blocker) = sunshine.blockers.first() {
            return Err(RemotePlayError::new("sunshine_unsafe", blocker.clone()));
        }
        if sunshine.upnp_enabled {
            return Err(RemotePlayError::new(
                "sunshine_upnp_active",
                "Sunshine already manages its own UPnP mapping (upnp = on). Turn it off in sunshine.conf to let Ludex manage the ports, or use Sunshine's mapping.",
            ));
        }
        let lan = local_ipv4().ok_or_else(|| {
            RemotePlayError::new(
                "no_lan_address",
                "Ludex could not detect a private LAN address for this machine, so a mapping would point nowhere.",
            )
        })?;
        if !is_private_ipv4(lan) {
            return Err(RemotePlayError::new(
                "no_lan_address",
                format!("The detected address {lan} is not a private LAN address; UPnP mapping needs one."),
            ));
        }
        let mappings = plan_mappings(&plan, &lan.to_string());
        let now = now_unix();
        let active = match apply_mappings(&gateway, &mappings, now) {
            Ok(active) => active,
            Err(failure) => {
                let rollback = remove_mappings(&gateway, &failure.applied);
                let leftovers: Vec<ActiveMapping> = failure
                    .applied
                    .iter()
                    .filter(|mapping| {
                        rollback.ports.iter().any(|port| {
                            port.protocol == mapping.protocol
                                && port.port == mapping.external_port
                                && port.status == "failed"
                        })
                    })
                    .cloned()
                    .collect();
                // A port the router refused to delete stays open until its
                // lease expires. Record it so disable and the startup sweep
                // can retry with the router instead of losing track of it.
                if !leftovers.is_empty() {
                    let leftover_session = RemotePlaySession {
                        gateway: Some(device.clone()),
                        mappings: leftovers,
                        public_ip: Some(wan.to_string()),
                        lan_ip: Some(lan.to_string()),
                        updated_at: Some(crate::util::now_rfc3339()),
                    };
                    let _ = save_session(&path, &leftover_session);
                    if let Ok(mut current) = shared.lock() {
                        *current = leftover_session;
                    }
                }
                let suffix = if rollback.complete {
                    " No mapping was left behind.".to_string()
                } else {
                    format!(" {} mapping(s) could not be rolled back; close the wizard to retry the cleanup.", rollback.ports.iter().filter(|port| port.status == "failed").count())
                };
                return Err(RemotePlayError::new(
                    &failure.error.code,
                    format!("{}{suffix}", failure.error.message),
                ));
            }
        };
        let probe = TcpProbe {
            timeout: Duration::from_millis(1500),
        };
        let verification = audit_mappings(
            &gateway,
            &plan,
            &active,
            Some(wan),
            Some(&lan.to_string()),
            &probe,
        );
        if !verification.verified {
            let rollback = remove_mappings(&gateway, &active);
            let leftovers: Vec<ActiveMapping> = active
                .iter()
                .filter(|mapping| {
                    rollback.ports.iter().any(|port| {
                        port.protocol == mapping.protocol
                            && port.port == mapping.external_port
                            && port.status == "failed"
                    })
                })
                .cloned()
                .collect();
            if !leftovers.is_empty() {
                let leftover_session = RemotePlaySession {
                    gateway: Some(device.clone()),
                    mappings: leftovers,
                    public_ip: Some(wan.to_string()),
                    lan_ip: Some(lan.to_string()),
                    updated_at: Some(crate::util::now_rfc3339()),
                };
                let _ = save_session(&path, &leftover_session);
                if let Ok(mut current) = shared.lock() {
                    *current = leftover_session;
                }
            }
            let suffix = if rollback.complete {
                " Every mapping was removed again.".to_string()
            } else {
                " Some mappings could not be removed; close the wizard to retry the cleanup and note that the lease expires automatically.".to_string()
            };
            return Err(RemotePlayError::new(
                "verification_failed",
                format!("{}{suffix}", verification.message),
            ));
        }
        let session = RemotePlaySession {
            gateway: Some(device),
            mappings: active.clone(),
            public_ip: Some(wan.to_string()),
            lan_ip: Some(lan.to_string()),
            updated_at: Some(crate::util::now_rfc3339()),
        };
        if let Err(error) = save_session(&path, &session) {
            // Without the bookkeeping file the ports could never be cleaned
            // up by a later run, so fail closed and remove them now.
            let rollback = remove_mappings(&gateway, &session.mappings);
            let suffix = if rollback.complete {
                " Every mapping was removed again."
            } else {
                " Some mappings could not be removed; the lease expires automatically."
            };
            return Err(RemotePlayError::new(
                &error.code,
                format!("{}{suffix}", error.message),
            ));
        }
        match shared.lock() {
            Ok(mut current) => *current = session,
            Err(poisoned) => *poisoned.into_inner() = session,
        }
        // Audit trail: ports, lease and address only; no PIN, token or secret.
        tracing::info!(
            ports = plan.total,
            lease_secs = plan.lease_secs,
            public_ip = %wan,
            "remote play direct mode enabled"
        );
        Ok(RemotePlayReport {
            enabled: true,
            public_ip: Some(wan.to_string()),
            lan_ip: Some(lan.to_string()),
            lease_secs: plan.lease_secs,
            mappings: active,
            verification,
            message: format!(
                "Direct mode is on: {} ports are mapped for {} seconds. Disable it when you finish; the mapping is also removed at the next start.",
                plan.total, plan.lease_secs
            ),
        })
    })
    .await
    .map_err(|error| {
        RemotePlayError::new(
            "mapping_failed",
            format!("The mapping task failed: {error}"),
        )
    })?
}

#[tauri::command]
pub async fn remote_play_verify(
    app: tauri::AppHandle,
) -> Result<VerificationReport, RemotePlayError> {
    use tauri::Manager;
    let remote = app.state::<RemotePlayState>();
    let session = remote.snapshot();
    if !session.is_active() {
        return Err(RemotePlayError::new(
            "not_mapped",
            "Direct mode is off: there is no mapping to verify.",
        ));
    }
    let Some(device) = session.gateway.clone() else {
        return Err(RemotePlayError::new(
            "not_mapped",
            "The router that was mapped is no longer known. Disable direct mode and start again.",
        ));
    };
    tauri::async_runtime::spawn_blocking(move || {
        let gateway = UpnpGateway::new(device, HttpTransport::new(Duration::from_secs(4)));
        let wan = gateway.external_ip().ok();
        let plan = PortPlan {
            tcp: TCP_PORTS.to_vec(),
            udp: {
                let mut udp = UDP_PORTS.to_vec();
                if session.mappings.iter().any(|mapping| {
                    mapping.protocol == PortProtocol::Udp
                        && mapping.external_port == OPTIONAL_UDP_PORT
                }) {
                    udp.push(OPTIONAL_UDP_PORT);
                }
                udp.sort_unstable();
                udp
            },
            lease_secs: session
                .mappings
                .first()
                .map(|mapping| mapping.lease_secs)
                .unwrap_or(DEFAULT_LEASE_SECS),
            microphone: session
                .mappings
                .iter()
                .any(|mapping| mapping.external_port == OPTIONAL_UDP_PORT),
            total: session.mappings.len(),
            never_mapped: FORBIDDEN_PORTS.to_vec(),
            warnings: Vec::new(),
        };
        let probe = TcpProbe {
            timeout: Duration::from_millis(1500),
        };
        Ok(audit_mappings(
            &gateway,
            &plan,
            &session.mappings,
            wan,
            session.lan_ip.as_deref(),
            &probe,
        ))
    })
    .await
    .map_err(|error| {
        RemotePlayError::new(
            "verification_failed",
            format!("The verification task failed: {error}"),
        )
    })?
}

/// Refresh mappings that are close to their lease expiry.
#[tauri::command]
pub async fn remote_play_refresh(
    app: tauri::AppHandle,
) -> Result<RemotePlayReport, RemotePlayError> {
    use tauri::Manager;
    let remote = app.state::<RemotePlayState>();
    let (path, shared) = remote.shared();
    let session = remote.snapshot();
    if !session.is_active() {
        return Err(RemotePlayError::new(
            "not_mapped",
            "Direct mode is off: there is no mapping to refresh.",
        ));
    }
    let Some(device) = session.gateway.clone() else {
        return Err(RemotePlayError::new(
            "not_mapped",
            "The router that was mapped is no longer known.",
        ));
    };
    let lease_secs = session
        .mappings
        .first()
        .map(|mapping| mapping.lease_secs)
        .unwrap_or(DEFAULT_LEASE_SECS);
    let due = leases_due(&session, now_unix(), lease_secs);
    if due.is_empty() {
        return Ok(RemotePlayReport {
            enabled: true,
            public_ip: session.public_ip.clone(),
            lan_ip: session.lan_ip.clone(),
            lease_secs,
            mappings: session.mappings.clone(),
            verification: VerificationReport {
                verified: true,
                public_ip: session.public_ip.clone(),
                lan_ip: session.lan_ip.clone(),
                reachable: None,
                ports: Vec::new(),
                notes: Vec::new(),
                message: "No mapping needed a refresh.".to_string(),
            },
            message: "No mapping needed a refresh yet.".to_string(),
        });
    }
    tauri::async_runtime::spawn_blocking(move || {
        let gateway = UpnpGateway::new(device, HttpTransport::new(Duration::from_secs(4)));
        let now = now_unix();
        let refreshed = match apply_mappings(&gateway, &due, now) {
            Ok(active) => active,
            Err(failure) => {
                let _ = remove_mappings(&gateway, &failure.applied);
                return Err(failure.error);
            }
        };
        let mut mappings = session.mappings.clone();
        for mapping in &refreshed {
            if let Some(existing) = mappings.iter_mut().find(|item| {
                item.protocol == mapping.protocol && item.external_port == mapping.external_port
            }) {
                *existing = mapping.clone();
            }
        }
        let public_ip = session
            .public_ip
            .as_deref()
            .and_then(|value| value.parse::<IpAddr>().ok());
        let lan_ip = session.lan_ip.clone();
        let verification = audit_mappings(
            &gateway,
            &plan_ports(
                mappings
                    .iter()
                    .any(|mapping| mapping.external_port == OPTIONAL_UDP_PORT),
                Some(lease_secs),
            ),
            &mappings,
            public_ip,
            lan_ip.as_deref(),
            &TcpProbe {
                timeout: Duration::from_millis(1500),
            },
        );
        let next = RemotePlaySession {
            mappings: mappings.clone(),
            updated_at: Some(crate::util::now_rfc3339()),
            ..session
        };
        save_session(&path, &next)?;
        match shared.lock() {
            Ok(mut current) => *current = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }
        Ok(RemotePlayReport {
            enabled: true,
            public_ip: verification.public_ip.clone(),
            lan_ip: verification.lan_ip.clone(),
            lease_secs,
            mappings,
            verification,
            message: "Port mappings were refreshed before their lease expired.".to_string(),
        })
    })
    .await
    .map_err(|error| {
        RemotePlayError::new(
            "mapping_failed",
            format!("The refresh task failed: {error}"),
        )
    })?
}

#[tauri::command]
pub async fn remote_play_disable(app: tauri::AppHandle) -> Result<CleanupReport, RemotePlayError> {
    use tauri::Manager;
    let remote = app.state::<RemotePlayState>();
    let (path, shared) = remote.shared();
    let session = remote.snapshot();
    if !session.is_active() {
        return Ok(CleanupReport {
            complete: true,
            removed: 0,
            ports: Vec::new(),
            message: "Direct mode is already off.".to_string(),
        });
    }
    let Some(device) = session.gateway.clone() else {
        clear_session(&path);
        return Ok(CleanupReport {
            complete: false,
            removed: 0,
            ports: Vec::new(),
            message: "Ludex no longer knows which router holds the mappings, so it cannot remove them. They expire with their lease; check the router's UPnP page to remove them now."
                .to_string(),
        });
    };
    tauri::async_runtime::spawn_blocking(move || {
        let gateway = UpnpGateway::new(device, HttpTransport::new(Duration::from_secs(4)));
        let report = remove_mappings(&gateway, &session.mappings);
        tracing::info!(
            removed = report.removed,
            complete = report.complete,
            "remote play direct mode disabled"
        );
        if report.complete {
            clear_session(&path);
            if let Ok(mut current) = shared.lock() {
                *current = RemotePlaySession::default();
            }
        } else {
            let remaining: Vec<ActiveMapping> = session
                .mappings
                .iter()
                .filter(|mapping| {
                    report.ports.iter().any(|port| {
                        port.protocol == mapping.protocol
                            && port.port == mapping.external_port
                            && port.status == "failed"
                    })
                })
                .cloned()
                .collect();
            let next = RemotePlaySession {
                gateway: session.gateway.clone(),
                mappings: remaining,
                public_ip: session.public_ip.clone(),
                lan_ip: session.lan_ip.clone(),
                updated_at: Some(crate::util::now_rfc3339()),
            };
            let _ = save_session(&path, &next);
            if let Ok(mut current) = shared.lock() {
                *current = next;
            }
        }
        Ok(report)
    })
    .await
    .map_err(|error| {
        RemotePlayError::new(
            "cleanup_failed",
            format!("The cleanup task failed: {error}"),
        )
    })?
}

/* --------------------------------- tests ---------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn mapping(protocol: PortProtocol, port: u16) -> PortMapping {
        PortMapping {
            protocol,
            external_port: port,
            internal_port: port,
            internal_client: "192.168.1.20".to_string(),
            lease_secs: 3600,
            description: mapping_description(protocol, port),
        }
    }

    #[test]
    fn plan_contains_exactly_the_moonlight_ports() {
        let plan = plan_ports(false, None);
        assert_eq!(plan.tcp, vec![47984, 47989, 48010]);
        assert_eq!(plan.udp, vec![47998, 47999, 48000, 48010]);
        assert_eq!(plan.total, 7);
        assert_eq!(plan.lease_secs, DEFAULT_LEASE_SECS);
        assert!(!plan.tcp.contains(&47990));
        assert!(!plan.udp.contains(&5353));

        let plan = plan_ports(true, Some(300));
        assert_eq!(plan.udp, vec![47998, 47999, 48000, 48002, 48010]);
        // The lease is clamped instead of trusted.
        assert_eq!(plan.lease_secs, MIN_LEASE_SECS);
        assert!(validate_plan(&plan).is_ok());
    }

    #[test]
    fn a_forged_plan_is_refused() {
        let mut plan = plan_ports(false, None);
        plan.tcp.push(47990);
        assert_eq!(validate_plan(&plan).unwrap_err().code, "port_plan_invalid");
        let mut plan = plan_ports(false, None);
        plan.udp.push(5353);
        assert!(validate_plan(&plan).is_err());
        let mut plan = plan_ports(false, None);
        plan.lease_secs = 10;
        assert_eq!(validate_plan(&plan).unwrap_err().code, "lease_invalid");
        let mut plan = plan_ports(false, None);
        plan.udp.push(12345);
        assert!(validate_plan(&plan).is_err());
        let mut plan = plan_ports(false, None);
        plan.total = MAX_REMOTE_PORTS + 1;
        assert!(validate_plan(&plan).is_err());
        let mut plan = plan_ports(false, None);
        plan.tcp.push(48010);
        plan.total += 1;
        assert_eq!(validate_plan(&plan).unwrap_err().code, "port_plan_invalid");
        let mut plan = plan_ports(false, None);
        plan.total = 1;
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn mapping_builders_carry_the_exact_ports_and_lease() {
        let plan = plan_ports(true, Some(1800));
        let mappings = plan_mappings(&plan, "192.168.1.20");
        assert_eq!(mappings.len(), 8);
        let tcp = mappings
            .iter()
            .find(|m| m.protocol == PortProtocol::Tcp && m.external_port == 47984)
            .unwrap();
        let body = add_port_mapping_body("urn:schemas-upnp-org:service:WANIPConnection:1", tcp);
        assert!(body.contains("<NewExternalPort>47984</NewExternalPort>"));
        assert!(body.contains("<NewProtocol>TCP</NewProtocol>"));
        assert!(body.contains("<NewInternalClient>192.168.1.20</NewInternalClient>"));
        assert!(body.contains("<NewLeaseDuration>1800</NewLeaseDuration>"));
        assert!(body.contains("AddPortMapping"));
        assert!(!body.contains("47990"));

        let udp = mappings
            .iter()
            .find(|m| m.external_port == OPTIONAL_UDP_PORT)
            .unwrap();
        assert!(add_port_mapping_body("urn:test", udp).contains("<NewProtocol>UDP</NewProtocol>"));

        let delete = delete_port_mapping_body("urn:test", PortProtocol::Udp, 47998);
        assert!(delete.contains("<NewExternalPort>47998</NewExternalPort>"));
        assert!(delete.contains("<NewProtocol>UDP</NewProtocol>"));
        assert!(delete.contains("DeletePortMapping"));
        assert!(
            get_specific_port_mapping_body("urn:test", PortProtocol::Tcp, 47989)
                .contains("GetSpecificPortMappingEntry")
        );
        assert!(get_external_ip_body("urn:test").contains("GetExternalIPAddress"));
    }

    #[test]
    fn descriptions_are_short_and_marked_as_ludex() {
        for mapping in plan_mappings(&plan_ports(true, None), "10.0.0.2") {
            assert!(mapping.description.len() <= 64);
            assert!(is_ludex_mapping(&mapping.description));
        }
        assert!(!is_ludex_mapping("qBittorrent"));
    }

    #[test]
    fn soap_responses_are_parsed_without_a_xml_dependency() {
        let external = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetExternalIPAddressResponse xmlns:u="urn:x"><NewExternalIPAddress>203.0.113.9</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body></s:Envelope>"#;
        assert_eq!(
            parse_external_ip(external).unwrap().to_string(),
            "203.0.113.9"
        );

        let entry = r#"<NewInternalPort>47989</NewInternalPort><NewInternalClient>192.168.1.20</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>Ludex remote play TCP 47989</NewPortMappingDescription><NewLeaseDuration>3600</NewLeaseDuration>"#;
        let parsed = parse_mapping_entry(entry).unwrap();
        assert_eq!(parsed.internal_port, 47989);
        assert_eq!(parsed.lease_secs, 3600);
        assert!(parsed.enabled);
        assert!(is_ludex_mapping(&parsed.description));

        let fault = r#"<s:Fault><detail><UPnPError><errorCode>714</errorCode><errorDescription>NoSuchEntryInArray</errorDescription></UPnPError></detail></s:Fault>"#;
        assert_eq!(parse_fault_code(fault), Some(714));
        assert_eq!(parse_fault_code(external), None);
    }

    #[test]
    fn igd_description_finds_the_wan_ip_service_and_resolves_urls() {
        let xml = r#"<root><device>
          <friendlyName>Home Router</friendlyName>
          <modelName>X1</modelName>
          <deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
            <serviceList>
              <service><serviceType>urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1</serviceType><controlURL>/upnp/control/common</controlURL></service>
              <service><serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType><controlURL>ppp/control</controlURL></service>
              <service><serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType><controlURL>/upnp/control/wanip</controlURL></service>
            </serviceList>
          </device></deviceList>
        </device></root>"#;
        let device = parse_igd_description(xml, "http://192.168.1.1:49152/rootDesc.xml").unwrap();
        assert_eq!(device.friendly_name, "Home Router");
        assert_eq!(device.model, "X1");
        assert_eq!(
            device.service_type,
            "urn:schemas-upnp-org:service:WANIPConnection:1"
        );
        assert_eq!(
            device.control_url,
            "http://192.168.1.1:49152/upnp/control/wanip"
        );

        assert_eq!(
            resolve_url("http://192.168.1.1:49152/desc/root.xml", "ctl"),
            "http://192.168.1.1:49152/desc/ctl"
        );
        assert_eq!(
            resolve_url("http://192.168.1.1/root.xml", "/ctl"),
            "http://192.168.1.1/ctl"
        );
        assert_eq!(
            resolve_url("http://192.168.1.1/root.xml", "http://10.0.0.1/ctl"),
            "http://10.0.0.1/ctl"
        );
    }

    #[test]
    fn ssdp_location_parsing_is_case_insensitive() {
        let packet = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nLOCATION: http://192.168.1.1:5000/rootDesc.xml\r\nST: urn:x\r\n\r\n";
        assert_eq!(
            parse_ssdp_location(packet).as_deref(),
            Some("http://192.168.1.1:5000/rootDesc.xml")
        );
        assert_eq!(
            parse_ssdp_location("location: http://10.0.0.1/d.xml\r\n"),
            Some("http://10.0.0.1/d.xml".to_string())
        );
        assert_eq!(parse_ssdp_location("SERVER: Linux\r\n"), None);
        // A non-HTTP location is never used.
        assert_eq!(
            parse_ssdp_location("LOCATION: file:///etc/passwd\r\n"),
            None
        );
    }

    #[test]
    fn private_locations_are_required_for_an_igd() {
        assert!(location_is_private("http://192.168.1.1:5000/rootDesc.xml"));
        assert!(location_is_private("http://10.0.0.1/rootDesc.xml"));
        assert!(location_is_private("http://172.16.4.1/rootDesc.xml"));
        assert!(!location_is_private("http://203.0.113.5/rootDesc.xml"));
        assert!(location_is_private("http://127.0.0.1/rootDesc.xml"));
        assert!(!location_is_private("file:///etc/passwd"));
    }

    #[test]
    fn cgnat_and_double_nat_are_refused_with_a_specific_reason() {
        let cgnat = assess_direct_mode(
            true,
            Some("100.64.0.1".parse().unwrap()),
            Some("203.0.113.9".parse().unwrap()),
        );
        assert!(!cgnat.allowed);
        assert_eq!(cgnat.code, "cgnat");
        assert!(cgnat.message.contains("100.64.0.0/10"));
        assert!(cgnat.vpn_recommended);

        let double = assess_direct_mode(
            true,
            Some("192.168.1.1".parse().unwrap()),
            Some("203.0.113.9".parse().unwrap()),
        );
        assert!(!double.allowed);
        assert_eq!(double.code, "double_nat");

        let mismatch = assess_direct_mode(
            true,
            Some("198.51.100.4".parse().unwrap()),
            Some("203.0.113.9".parse().unwrap()),
        );
        assert!(!mismatch.allowed);
        assert_eq!(mismatch.code, "double_nat");
        assert!(mismatch.message.contains("198.51.100.4"));

        let no_igd = assess_direct_mode(false, None, None);
        assert_eq!(no_igd.code, "upnp_unavailable");

        let ok = assess_direct_mode(
            true,
            Some("198.51.100.4".parse().unwrap()),
            Some("198.51.100.4".parse().unwrap()),
        );
        assert!(ok.allowed);
        assert_eq!(ok.code, "ok");
    }

    #[test]
    fn only_global_ipv6_counts_as_routable() {
        assert!(is_global_ipv6("2001:db8::1".parse().unwrap()));
        assert!(is_global_ipv6("2a00:1450:4001::1".parse().unwrap()));
        assert!(!is_global_ipv6("fe80::1".parse().unwrap()));
        assert!(!is_global_ipv6("fd00::1".parse().unwrap()));
        assert!(!is_global_ipv6("::1".parse().unwrap()));
    }

    #[test]
    fn address_classes_cover_cgnat_private_and_public() {
        assert_eq!(
            classify_wan("100.127.255.254".parse().unwrap()),
            WanKind::Cgnat
        );
        assert_eq!(
            classify_wan("100.128.0.1".parse().unwrap()),
            WanKind::Public
        );
        assert_eq!(
            classify_wan("192.168.0.10".parse().unwrap()),
            WanKind::Private
        );
        assert_eq!(
            classify_wan("198.51.100.9".parse().unwrap()),
            WanKind::Public
        );
        assert_eq!(classify_wan("::1".parse().unwrap()), WanKind::Private);
    }

    #[test]
    fn sunshine_conf_is_read_only_and_unsafe_values_block_mapping() {
        let conf = parse_sunshine_conf(
            "# comment\nport = 47989\norigin_web_ui_allowed = wan\norigin_pin_allowed = pc\nwan_encryption_mode = 0\nupnp = on\n",
        );
        assert_eq!(sunshine_port_base(&conf), 47989);
        let (warnings, blockers) = sunshine_readiness(&conf);
        assert_eq!(blockers.len(), 2);
        assert!(blockers.iter().any(|value| value.contains("Web UI")));
        assert!(blockers.iter().any(|value| value.contains("encryption")));
        assert!(warnings.iter().any(|value| value.contains("UPnP")));

        let safe = parse_sunshine_conf(
            "origin_web_ui_allowed = lan\norigin_pin_allowed = pc\nwan_encryption_mode = 1\n",
        );
        let (warnings, blockers) = sunshine_readiness(&safe);
        assert!(blockers.is_empty());
        assert!(warnings.is_empty());

        let defaults = parse_sunshine_conf("");
        let (warnings, blockers) = sunshine_readiness(&defaults);
        assert!(blockers.is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn sunshine_web_ports_follow_the_base_port() {
        assert_eq!(sunshine_web_ports(47989), (47984, 47989, 47990));
        assert_eq!(sunshine_web_ports(8), (3, 8, 9));
    }

    fn fixture(interface: &str, address: &str) -> InterfaceAddress {
        let (addr, prefix) = address
            .split_once('/')
            .expect("fixture addresses carry a prefix");
        InterfaceAddress {
            interface: interface.to_string(),
            address: addr.parse().expect("fixture address parses"),
            prefix: prefix.parse().expect("fixture prefix parses"),
        }
    }

    fn classified(entries: &[(&str, &str)]) -> Vec<LocalAddress> {
        classify_addresses(
            &entries
                .iter()
                .map(|(interface, address)| fixture(interface, address))
                .collect::<Vec<_>>(),
        )
    }

    fn vpn_for(entries: &[(&str, &str)], process_names: &[&str]) -> (Vec<LocalAddress>, VpnStatus) {
        vpn_for_with_interfaces(
            entries,
            &entries.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            process_names,
        )
    }

    fn vpn_for_with_interfaces(
        entries: &[(&str, &str)],
        interfaces: &[&str],
        process_names: &[&str],
    ) -> (Vec<LocalAddress>, VpnStatus) {
        let addresses = classified(entries);
        let interfaces = interfaces
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let names = process_names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let vpn = vpn_status(&names, &addresses, &interfaces);
        (addresses, vpn)
    }

    fn kind_of<'a>(addresses: &'a [LocalAddress], address: &str) -> &'a AddressKind {
        &addresses
            .iter()
            .find(|entry| entry.address == address)
            .expect("fixture address is classified")
            .kind
    }

    #[test]
    fn interfaces_are_classified_by_name_and_range() {
        let addresses = classified(&[
            ("lo", "127.0.0.1/8"),
            ("enp3s0", "192.168.1.8/24"),
            ("docker0", "172.17.0.1/16"),
            ("veth1a2b", "169.254.10.2/16"),
            ("eth0", "100.64.3.7/10"),
            ("wlan0", "2001:db8::8/64"),
            ("wlan0", "fe80::8/64"),
            ("enp3s0", "fd00::8/64"),
        ]);
        assert_eq!(kind_of(&addresses, "127.0.0.1"), &AddressKind::Loopback);
        assert_eq!(kind_of(&addresses, "192.168.1.8"), &AddressKind::Lan);
        assert_eq!(kind_of(&addresses, "172.17.0.1"), &AddressKind::Virtual);
        assert_eq!(kind_of(&addresses, "169.254.10.2"), &AddressKind::LinkLocal);
        assert_eq!(kind_of(&addresses, "100.64.3.7"), &AddressKind::Cgnat);
        assert_eq!(kind_of(&addresses, "2001:db8::8"), &AddressKind::Ipv6);
        assert_eq!(kind_of(&addresses, "fe80::8"), &AddressKind::LinkLocal);
        assert_eq!(kind_of(&addresses, "fd00::8"), &AddressKind::Ula);
        // VPN adapters are matched by name, whatever the address range.
        assert_eq!(vpn_interface_service("wt0"), Some("Netbird"));
        assert_eq!(vpn_interface_service("Tailscale"), Some("Tailscale"));
        assert_eq!(vpn_interface_service("wg0"), Some("WireGuard"));
        assert_eq!(vpn_interface_service("zt9a1b2c3"), Some("ZeroTier"));
        assert_eq!(vpn_interface_service("utun3"), Some("Tunnel"));
        assert_eq!(vpn_interface_service("enp3s0"), None);
        assert!(is_virtual_interface("docker0"));
        assert!(is_virtual_interface("vEthernet (WSL)"));
        assert!(is_virtual_interface("virbr0"));
        assert!(!is_virtual_interface("enp3s0"));
    }

    #[test]
    fn netbird_address_wins_over_the_lan_address() {
        let (addresses, vpn) = vpn_for(
            &[("enp3s0", "192.168.1.8/24"), ("wt0", "100.101.102.103/16")],
            &["netbird"],
        );
        assert!(vpn.detected);
        assert_eq!(vpn.services, vec!["Netbird"]);
        assert_eq!(vpn.address.as_deref(), Some("100.101.102.103"));
        assert_eq!(vpn.addresses.len(), 1);
        assert_eq!(vpn.addresses[0].interface, "wt0");
        assert!(vpn.interfaces.is_empty());

        let advertised = select_advertised(&addresses, &vpn, Some("192.168.1.8".parse().unwrap()));
        assert_eq!(advertised.address.as_deref(), Some("100.101.102.103"));
        assert_eq!(advertised.source, "vpn");
        assert_eq!(advertised.service.as_deref(), Some("Netbird"));
        assert!(advertised.reason.contains("Netbird"));
        assert!(advertised.reason.contains("100.101.102.103"));
    }

    #[test]
    fn a_tunnel_interface_is_enough_without_its_process() {
        let (_, vpn) = vpn_for(
            &[
                ("enp3s0", "192.168.1.8/24"),
                ("tailscale0", "100.64.0.5/32"),
            ],
            &[],
        );
        assert!(vpn.detected);
        assert_eq!(vpn.services, vec!["Tailscale"]);
        assert_eq!(vpn.address.as_deref(), Some("100.64.0.5"));
    }

    #[test]
    fn wireguard_private_address_wins_over_the_lan() {
        let (addresses, vpn) =
            vpn_for(&[("enp3s0", "192.168.1.8/24"), ("wg0", "10.8.0.2/24")], &[]);
        assert_eq!(vpn.services, vec!["WireGuard"]);
        assert_eq!(vpn.address.as_deref(), Some("10.8.0.2"));
        let advertised = select_advertised(&addresses, &vpn, Some("192.168.1.8".parse().unwrap()));
        assert_eq!(advertised.address.as_deref(), Some("10.8.0.2"));
        assert_eq!(advertised.source, "vpn");
    }

    #[test]
    fn no_vpn_falls_back_to_the_lan_and_never_a_bridge() {
        let (addresses, vpn) = vpn_for(
            &[("enp3s0", "192.168.1.8/24"), ("docker0", "172.17.0.1/16")],
            &["sunshine"],
        );
        assert!(!vpn.detected);
        assert!(vpn.address.is_none());
        let advertised = select_advertised(&addresses, &vpn, Some("192.168.1.8".parse().unwrap()));
        assert_eq!(advertised.address.as_deref(), Some("192.168.1.8"));
        assert_eq!(advertised.source, "lan");
        assert!(advertised.reason.contains("only works at home"));
        assert!(!advertised.reason.contains("172.17.0.1"));
    }

    #[test]
    fn a_vpn_without_an_address_names_its_interface() {
        let (addresses, vpn) = vpn_for_with_interfaces(
            &[("enp3s0", "192.168.1.8/24")],
            &["enp3s0", "wt0"],
            &["netbird"],
        );
        assert!(vpn.detected);
        assert!(vpn.address.is_none());
        assert_eq!(vpn.interfaces, vec!["wt0"]);
        assert!(vpn.message.contains("wt0"));
        assert!(vpn.message.contains("no VPN address"));
        let advertised = select_advertised(&addresses, &vpn, Some("192.168.1.8".parse().unwrap()));
        assert_eq!(advertised.address.as_deref(), Some("192.168.1.8"));
        assert_eq!(advertised.source, "lan");
        assert!(advertised.reason.contains("no VPN address"));
    }

    #[test]
    fn cgnat_on_an_unknown_interface_is_never_a_vpn() {
        let (addresses, vpn) = vpn_for(&[("eth0", "100.64.3.7/10")], &[]);
        assert_eq!(kind_of(&addresses, "100.64.3.7"), &AddressKind::Cgnat);
        assert!(!vpn.detected);
        assert!(vpn.address.is_none());
        let advertised = select_advertised(&addresses, &vpn, Some("100.64.3.7".parse().unwrap()));
        assert!(advertised.address.is_none());
        assert_eq!(advertised.source, "none");
    }

    #[test]
    fn ipv6_is_a_fallback_only() {
        let (addresses, vpn) = vpn_for(
            &[("enp3s0", "192.168.1.8/24"), ("wlan0", "2001:db8::8/64")],
            &[],
        );
        let advertised = select_advertised(&addresses, &vpn, None);
        assert_eq!(advertised.address.as_deref(), Some("192.168.1.8"));

        let (only_v6, vpn) = vpn_for(&[("wlan0", "2001:db8::8/64")], &[]);
        let advertised = select_advertised(&only_v6, &vpn, None);
        assert_eq!(advertised.address.as_deref(), Some("2001:db8::8"));
        assert_eq!(advertised.source, "ipv6");
    }

    #[test]
    fn several_vpns_are_all_listed_and_the_mesh_wins() {
        let (addresses, vpn) = vpn_for(
            &[
                ("enp3s0", "192.168.1.8/24"),
                ("wg0", "10.8.0.2/24"),
                ("wt0", "100.101.102.103/16"),
            ],
            &["wireguard", "netbird"],
        );
        assert_eq!(vpn.addresses.len(), 2);
        assert_eq!(vpn.address.as_deref(), Some("100.101.102.103"));
        assert_eq!(vpn.addresses[1].service, "WireGuard");
        let advertised = select_advertised(&addresses, &vpn, Some("192.168.1.8".parse().unwrap()));
        assert_eq!(advertised.source, "vpn");
    }

    #[test]
    fn vpn_detection_names_the_services_and_tailnet_address() {
        let names = vec![
            "tailscaled".to_string(),
            "sunshine".to_string(),
            "zerotier-one".to_string(),
            "tailscaled".to_string(),
        ];
        let addresses = classified(&[("tailscale0", "100.101.102.103/32")]);
        let status = vpn_status(&names, &addresses, &["tailscale0".to_string()]);
        assert!(status.detected);
        assert_eq!(status.services, vec!["Tailscale", "ZeroTier"]);
        assert_eq!(status.address.as_deref(), Some("100.101.102.103"));
        assert!(status.message.contains("100.101.102.103"));

        let none = vpn_status(&["sunshine".to_string()], &[], &[]);
        assert!(!none.detected);
        assert!(none.message.contains("VPN"));
    }

    #[test]
    fn lease_bookkeeping_refreshes_before_expiry() {
        let session = RemotePlaySession {
            mappings: vec![ActiveMapping {
                protocol: PortProtocol::Tcp,
                external_port: 47984,
                internal_port: 47984,
                internal_client: "192.168.1.20".to_string(),
                lease_secs: 3600,
                description: mapping_description(PortProtocol::Tcp, 47984),
                expires_at: 10_000,
            }],
            ..RemotePlaySession::default()
        };
        assert!(leases_due(&session, 7_000, 3600).is_empty());
        assert_eq!(leases_due(&session, 9_000, 3600).len(), 1);
        assert_eq!(leases_due(&session, 10_500, 3600).len(), 1);
        assert_eq!(
            leases_due(&session, 9_000, 3600)[0].protocol,
            PortProtocol::Tcp
        );
    }

    #[test]
    fn session_round_trips_atomically_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "ludex-remote-play-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        let path = session_path(&dir);
        let session = RemotePlaySession {
            gateway: Some(IgdDevice {
                location: "http://192.168.1.1/rootDesc.xml".to_string(),
                control_url: "http://192.168.1.1/ctl".to_string(),
                service_type: "urn:schemas-upnp-org:service:WANIPConnection:1".to_string(),
                friendly_name: "Router".to_string(),
                model: "X1".to_string(),
            }),
            mappings: vec![ActiveMapping::from_mapping(
                &mapping(PortProtocol::Udp, 47998),
                1_000,
            )],
            public_ip: Some("198.51.100.4".to_string()),
            lan_ip: Some("192.168.1.20".to_string()),
            updated_at: Some("2026-11-12T00:00:00Z".to_string()),
        };
        save_session(&path, &session).unwrap();
        let loaded = load_session(&path);
        assert_eq!(loaded, session);
        assert!(path.is_file());
        assert!(!path.with_extension("json.tmp").exists());

        clear_session(&path);
        assert!(!path.exists());
        assert_eq!(load_session(&path), RemotePlaySession::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct MemoryMapper {
        entries: RefCell<BTreeMap<(PortProtocol, u16), RemoteMapping>>,
        fail_add_on: Option<u16>,
        fail_delete: bool,
    }

    impl MemoryMapper {
        fn new() -> Self {
            Self {
                entries: RefCell::new(BTreeMap::new()),
                fail_add_on: None,
                fail_delete: false,
            }
        }

        fn own(&self, protocol: PortProtocol, port: u16) {
            self.entries.borrow_mut().insert(
                (protocol, port),
                RemoteMapping {
                    internal_client: "192.168.1.20".to_string(),
                    internal_port: port,
                    description: mapping_description(protocol, port),
                    enabled: true,
                    lease_secs: 3600,
                },
            );
        }
    }

    impl PortMapper for MemoryMapper {
        fn external_ip(&self) -> Result<IpAddr, RemotePlayError> {
            Ok("198.51.100.4".parse().unwrap())
        }

        fn add(&self, mapping: &PortMapping) -> Result<(), RemotePlayError> {
            if self.fail_add_on == Some(mapping.external_port) {
                return Err(RemotePlayError::new("upnp_fault", "simulated add failure"));
            }
            self.entries.borrow_mut().insert(
                (mapping.protocol, mapping.external_port),
                RemoteMapping {
                    internal_client: mapping.internal_client.clone(),
                    internal_port: mapping.internal_port,
                    description: mapping.description.clone(),
                    enabled: true,
                    lease_secs: mapping.lease_secs,
                },
            );
            Ok(())
        }

        fn delete(&self, protocol: PortProtocol, port: u16) -> Result<(), RemotePlayError> {
            if self.fail_delete {
                return Err(RemotePlayError::new(
                    "upnp_fault",
                    "simulated delete failure",
                ));
            }
            self.entries.borrow_mut().remove(&(protocol, port));
            Ok(())
        }

        fn get(
            &self,
            protocol: PortProtocol,
            port: u16,
        ) -> Result<Option<RemoteMapping>, RemotePlayError> {
            Ok(self.entries.borrow().get(&(protocol, port)).cloned())
        }
    }

    struct AlwaysReachable;

    impl Reachability for AlwaysReachable {
        fn tcp(&self, _address: SocketAddr) -> bool {
            true
        }
    }

    struct NeverReachable;

    impl Reachability for NeverReachable {
        fn tcp(&self, _address: SocketAddr) -> bool {
            false
        }
    }

    #[test]
    fn apply_refuses_a_port_owned_by_another_program() {
        let mapper = MemoryMapper::new();
        mapper.entries.borrow_mut().insert(
            (PortProtocol::Tcp, 47984),
            RemoteMapping {
                internal_client: "192.168.1.50".to_string(),
                internal_port: 47984,
                description: "my own forward".to_string(),
                enabled: true,
                lease_secs: 0,
            },
        );
        let plan = plan_ports(false, None);
        let mappings = plan_mappings(&plan, "192.168.1.20");
        let failure = apply_mappings(&mapper, &mappings, 1_000).unwrap_err();
        assert_eq!(failure.error.code, "port_owned");
        assert!(failure.error.message.contains("192.168.1.50"));
        // Nothing after the conflicting port was attempted and nothing was
        // overwritten.
        assert_eq!(mapper.entries.borrow().len(), 1);
        assert_eq!(
            mapper
                .entries
                .borrow()
                .get(&(PortProtocol::Tcp, 47984))
                .unwrap()
                .internal_client,
            "192.168.1.50"
        );
        assert!(failure.applied.is_empty());
    }

    #[test]
    fn a_mid_flow_failure_reports_what_to_roll_back() {
        let mut mapper = MemoryMapper::new();
        mapper.fail_add_on = Some(47999);
        let plan = plan_ports(true, None);
        let mappings = plan_mappings(&plan, "192.168.1.20");
        let failure = apply_mappings(&mapper, &mappings, 1_000).unwrap_err();
        assert_eq!(failure.error.code, "upnp_fault");
        assert_eq!(failure.applied.len(), 4);
        let cleanup = remove_mappings(&mapper, &failure.applied);
        assert!(cleanup.complete);
        assert!(mapper.entries.borrow().is_empty());
    }

    #[test]
    fn a_refresh_of_a_ludex_mapping_is_allowed() {
        let mapper = MemoryMapper::new();
        mapper.own(PortProtocol::Tcp, 47984);
        let plan = plan_ports(false, None);
        let mappings = plan_mappings(&plan, "192.168.1.20");
        let active = apply_mappings(&mapper, &mappings, 2_000).unwrap();
        assert_eq!(active.len(), 7);
        assert_eq!(active[0].expires_at, 5_600);
    }

    #[test]
    fn audit_reports_per_port_status_and_distinguishes_failures() {
        let mapper = MemoryMapper::new();
        let plan = plan_ports(false, None);
        let mappings = plan_mappings(&plan, "192.168.1.20");
        let active = apply_mappings(&mapper, &mappings, 0).unwrap();

        let reachable = audit_mappings(
            &mapper,
            &plan,
            &active,
            Some("198.51.100.4".parse().unwrap()),
            Some("192.168.1.20"),
            &AlwaysReachable,
        );
        assert!(reachable.verified);
        assert_eq!(reachable.reachable, Some(true));
        assert_eq!(reachable.ports.len(), 7);
        assert!(reachable
            .ports
            .iter()
            .filter(|port| port.protocol == PortProtocol::Tcp)
            .all(|port| port.status == "reachable"));
        assert!(reachable
            .ports
            .iter()
            .filter(|port| port.protocol == PortProtocol::Udp)
            .all(|port| port.status == "mapped" && port.reachable.is_none()));

        let loopback_blocked = audit_mappings(
            &mapper,
            &plan,
            &active,
            Some("198.51.100.4".parse().unwrap()),
            Some("192.168.1.20"),
            &NeverReachable,
        );
        // Mapped stays true even when the local probe cannot hairpin.
        assert!(loopback_blocked.verified);
        assert_eq!(loopback_blocked.reachable, Some(false));
        assert!(loopback_blocked
            .ports
            .iter()
            .any(|port| port.status == "mapped"));
        assert!(loopback_blocked
            .notes
            .iter()
            .any(|note| note.contains("NAT loopback")));

        // Remove one mapping: it is "not_mapped", not merely unreachable.
        mapper
            .entries
            .borrow_mut()
            .remove(&(PortProtocol::Tcp, 47989));
        let missing = audit_mappings(
            &mapper,
            &plan,
            &active,
            Some("198.51.100.4".parse().unwrap()),
            Some("192.168.1.20"),
            &AlwaysReachable,
        );
        assert!(!missing.verified);
        assert!(missing
            .ports
            .iter()
            .any(|port| port.port == 47989 && port.status == "not_mapped"));

        // A foreign mapping on a planned port is a conflict.
        mapper.entries.borrow_mut().insert(
            (PortProtocol::Udp, 48000),
            RemoteMapping {
                internal_client: "192.168.1.60".to_string(),
                internal_port: 48000,
                description: "other".to_string(),
                enabled: true,
                lease_secs: 0,
            },
        );
        let conflict = audit_mappings(
            &mapper,
            &plan,
            &active,
            None,
            Some("192.168.1.20"),
            &AlwaysReachable,
        );
        assert!(!conflict.verified);
        assert!(conflict
            .ports
            .iter()
            .any(|port| port.port == 48000 && port.status == "conflict"));
    }

    #[test]
    fn cleanup_removes_every_mapping_and_verifies_it() {
        let mapper = MemoryMapper::new();
        let plan = plan_ports(true, None);
        let mappings = plan_mappings(&plan, "192.168.1.20");
        let active = apply_mappings(&mapper, &mappings, 0).unwrap();
        let report = remove_mappings(&mapper, &active);
        assert!(report.complete);
        assert_eq!(report.removed, 8);
        assert!(report.ports.iter().all(|port| port.status == "removed"));
        assert!(mapper.entries.borrow().is_empty());

        let empty = remove_mappings(&mapper, &active);
        assert!(empty.complete);
        assert_eq!(empty.removed, 0);

        let failing = MemoryMapper {
            fail_delete: true,
            ..MemoryMapper::new()
        };
        for mapping in &active {
            failing.own(mapping.protocol, mapping.external_port);
        }
        let report = remove_mappings(&failing, &active);
        assert!(!report.complete);
        assert_eq!(
            report
                .ports
                .iter()
                .filter(|port| port.status == "failed")
                .count(),
            8
        );
    }

    #[test]
    fn cleanup_filters_only_ludex_ports_from_a_failed_delete() {
        let mapper = MemoryMapper::new();
        let plan = plan_ports(false, None);
        let mappings = plan_mappings(&plan, "192.168.1.20");
        let active = apply_mappings(&mapper, &mappings, 0).unwrap();
        // Simulate a router that refuses to delete one port.
        let mut partial = active.clone();
        partial.retain(|mapping| mapping.external_port != 47989);
        let report = remove_mappings(&mapper, &partial);
        assert!(report.complete);
        let remaining: Vec<_> = active
            .into_iter()
            .filter(|mapping| mapping.external_port == 47989)
            .collect();
        let report = remove_mappings(&mapper, &remaining);
        assert!(report.complete);
        assert!(mapper.entries.borrow().is_empty());
    }

    #[test]
    fn the_detection_snapshot_keeps_the_plan_limits_visible() {
        let status = detect(
            &LocalSettings::default(),
            &[],
            None,
            Some(RemotePlayError::new("upnp_unavailable", "no gateway")),
            None,
            RemotePlaySession::default(),
        );
        assert!(!status.upnp.available);
        assert!(!status.direct.allowed);
        assert_eq!(status.lease_min_secs, MIN_LEASE_SECS);
        assert_eq!(status.lease_max_secs, MAX_LEASE_SECS);
        assert!(status.message.contains("VPN") || status.message.contains("gateway"));
        let _ = status.platform;
    }

    #[test]
    fn xml_escaping_keeps_a_description_inside_the_document() {
        let mapping = PortMapping {
            description: "Ludex <&> test".to_string(),
            ..mapping(PortProtocol::Tcp, 47984)
        };
        let body = add_port_mapping_body("urn:test", &mapping);
        assert!(body.contains("Ludex &lt;&amp;&gt; test"));
        assert!(!body.contains("Ludex <&> test"));
    }
}
