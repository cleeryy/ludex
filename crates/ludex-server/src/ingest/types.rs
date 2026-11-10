//! Wire types, tuning knobs and pure helpers for the ingest manager.
//!
//! The JSON shape mirrors the `IngestJob` type from `docs/CONTRACT.md`; field
//! names are camelCase on the wire and must stay in sync with the web client.

use std::net::IpAddr;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

/// Longest title kept when sanitizing user input or inferred titles.
pub const MAX_TITLE_CHARS: usize = 120;
/// User agent sent with every ingest request.
pub const USER_AGENT: &str = "ludex/0.1";
/// Job history, persisted atomically next to the SQLite database.
pub const JOBS_FILE: &str = "ingest-jobs.json";
/// Job id -> `.part` file name mapping. Kept beside the job history so a restart
/// resumes the exact partial file even when two jobs share a title.
pub const PARTS_FILE: &str = "ingest-parts.json";
/// Suffix of in-progress downloads. No archive extension on purpose: the real
/// format is only known from the magic bytes once the download finishes.
pub const PART_SUFFIX: &str = ".part";

/// Lifecycle of an ingest download, matching the contract enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngestStatus {
    Queued,
    Downloading,
    Verifying,
    Done,
    Error,
    Cancelled,
}

impl IngestStatus {
    /// Stable lowercase label (matches the JSON representation).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Downloading => "downloading",
            Self::Verifying => "verifying",
            Self::Done => "done",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    /// Queued, downloading or verifying: a task owns the job right now.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Downloading | Self::Verifying)
    }

    /// Error or cancelled: a retry is allowed.
    pub fn is_stopped(self) -> bool {
        matches!(self, Self::Error | Self::Cancelled)
    }
}

/// One ingest download as returned by the API and persisted on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestJob {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub status: IngestStatus,
    pub file_name: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub speed_bps: u64,
    pub attempt: u32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Body of `POST /api/ingest/downloads`.
#[derive(Debug, Deserialize)]
pub struct CreateDownloadRequest {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// Body of `PUT /api/ingest/downloads/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateDownloadRequest {
    pub url: String,
}

/// Entry of `GET /api/ingest/files`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestFile {
    pub name: String,
    pub size: u64,
    /// True for `.{title}.part` downloads that can still be resumed.
    pub partial: bool,
    pub modified_at: Option<DateTime<Utc>>,
}

/// Response of `GET /api/ingest/stats`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestStats {
    pub games_dir: String,
    pub disk_total: u64,
    pub disk_free: u64,
    pub library_bytes: u64,
    pub game_count: u64,
    pub active_jobs: u64,
}

/// Response of `POST /api/ingest/files/{name}/verify`.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyResponse {
    pub ok: bool,
    pub log: String,
}

/// Runtime tuning. Defaults are production values; `Timings::fast` is used by
/// the unit tests so they never wait on the 90 s stall timeout or minute-scale
/// retry ladder.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub connect_timeout: Duration,
    /// Abort the attempt when no bytes arrived for this long.
    pub stall_timeout: Duration,
    /// How often progress (bytes, speed) is pushed to the job.
    pub progress_interval: Duration,
    /// How often buffered bytes are flushed to disk.
    pub flush_interval: Duration,
    /// Retry backoff ladder: 5 s, 10 s, 30 s, 60 s, then every 5 min.
    pub retry_delays: [Duration; 5],
    /// Concurrent downloads (the rest stay queued).
    pub max_active: usize,
    /// Best-effort archive check budget at the end of a download.
    pub verify_timeout: Duration,
    /// Write buffer between the HTTP stream and the file.
    pub buffer_size: usize,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(30),
            stall_timeout: Duration::from_secs(90),
            progress_interval: Duration::from_millis(500),
            flush_interval: Duration::from_secs(2),
            retry_delays: [
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(30),
                Duration::from_secs(60),
                Duration::from_secs(300),
            ],
            max_active: 3,
            verify_timeout: Duration::from_secs(120),
            buffer_size: 256 * 1024,
        }
    }
}

impl Timings {
    /// Delay before the next attempt. `step` is the number of consecutive
    /// failures since the last byte of progress.
    pub fn retry_delay(&self, step: u32) -> Duration {
        let index = (step as usize).min(self.retry_delays.len() - 1);
        self.retry_delays[index]
    }

    #[cfg(test)]
    pub(crate) fn fast() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            stall_timeout: Duration::from_secs(3),
            progress_interval: Duration::from_millis(20),
            flush_interval: Duration::from_millis(50),
            retry_delays: [Duration::from_millis(50); 5],
            max_active: 2,
            verify_timeout: Duration::from_secs(2),
            buffer_size: 64 * 1024,
        }
    }
}

/// Clean a user-supplied or inferred title for use as a file name stem.
///
/// Removes control characters and the Windows/POSIX separators, collapses
/// whitespace, caps the length and falls back to `Game`. Accents, apostrophes
/// and inner spaces are preserved (`Assassin's Creed Édition` stays readable).
pub fn sanitize_title(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut pending_space = false;
    for character in raw.chars() {
        if character.is_control()
            || matches!(
                character,
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            )
        {
            continue;
        }
        if character.is_whitespace() {
            pending_space = !cleaned.is_empty();
            continue;
        }
        if pending_space {
            cleaned.push(' ');
            pending_space = false;
        }
        cleaned.push(character);
    }
    let trimmed = cleaned
        .trim()
        .trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut title: String = trimmed.chars().take(MAX_TITLE_CHARS).collect();
    title = title.trim().to_string();
    if title.is_empty() {
        "Game".to_string()
    } else {
        title
    }
}

/// Infer a readable title from the last path segment of a URL.
pub fn infer_title(url: &Url) -> String {
    let segment = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let decoded = percent_decode(segment);
    let stem = strip_archive_extension(&decoded);
    let spaced = stem.replace(['_', '+'], " ");
    sanitize_title(&spaced)
}

fn strip_archive_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((stem, extension))
            if matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "rar" | "7z" | "part"
            ) =>
        {
            stem
        }
        _ => name,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse and validate the scheme/host part of a download URL.
///
/// Private address resolution is a separate, asynchronous step because it needs
/// DNS; this function only rejects URLs that can never be fetched.
pub fn parse_http_url(raw: &str) -> Result<Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL is empty".to_string());
    }
    let url = Url::parse(trimmed).map_err(|error| format!("Invalid URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http:// and https:// URLs are allowed".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URLs with embedded credentials are not allowed".to_string());
    }
    if url.host_str().is_none() {
        return Err("URL has no host".to_string());
    }
    Ok(url)
}

/// Loopback, private, link-local, unspecified and multicast addresses are
/// refused unless `LUDEX_INGEST_ALLOW_PRIVATE` opted in.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => {
            address.is_loopback()
                || address.is_private()
                || address.is_link_local()
                || address.is_unspecified()
                || address.is_multicast()
                || address.is_broadcast()
                // 100.64.0.0/10 carrier-grade NAT.
                || (address.octets()[0] == 100 && (64..=127).contains(&address.octets()[1]))
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(mapped));
            }
            address.is_loopback()
                || address.is_unspecified()
                || address.is_unique_local()
                || address.is_unicast_link_local()
                || address.is_multicast()
        }
    }
}

/// Archive extensions counted by the library stats and listed by the scanner.
pub fn is_archive_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.to_ascii_lowercase().as_str(),
            "zip" | "rar" | "7z"
        )
    })
}
