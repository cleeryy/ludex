//! Public desktop download metadata (#164).
//!
//! `GET /api/desktop/download` answers "where do I get the desktop app?" for
//! logged-out browsers. The server polls the stable GitHub release (or any
//! GitHub-compatible releases API configured with
//! `LUDEX_DESKTOP_RELEASES_API`), keeps the parsed result in memory for ten
//! minutes by default, and returns the asset URLs plus the release page.
//!
//! Nothing here proxies or mirrors a binary: the response only carries links,
//! sizes and checksums, so a self-hosted server never pays for the bytes. When
//! the upstream is offline, rate-limited or serving garbage, the endpoint still
//! answers `200` with a fallback document that only holds the releases page, so
//! the download page keeps working (and says so) instead of showing a broken
//! button.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::header::ACCEPT;
use serde::Deserialize;

use crate::config::{Config, DEFAULT_DESKTOP_RELEASES_PAGE};

/// Upper bound on the release document we are willing to read (512 KiB).
const MAX_RELEASE_BYTES: usize = 512 * 1024;
/// Upper bound on the checksum file we are willing to read (256 KiB).
const MAX_CHECKSUM_BYTES: usize = 256 * 1024;
/// Longest release-notes summary kept for the download page.
const MAX_SUMMARY_CHARS: usize = 320;

/// Platforms the download page offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Windows x64 installer (`setup.exe`, `.msi`).
    Windows,
    /// Linux AppImage, `.deb` or `.rpm`.
    Linux,
    /// macOS `.dmg` or `.pkg`.
    Macos,
}

impl Platform {
    /// Parses the `platform` query parameter, accepting the usual aliases.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "windows" | "win" | "win32" | "win64" => Some(Self::Windows),
            "linux" | "appimage" | "deb" | "rpm" => Some(Self::Linux),
            "macos" | "mac" | "osx" | "darwin" => Some(Self::Macos),
            _ => None,
        }
    }

    /// Canonical identifier used in the response.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Macos => "macos",
        }
    }
}

/// Kind of downloadable file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// Windows NSIS/MSI, macOS `.pkg` or a generic installer.
    Installer,
    /// Linux AppImage.
    Appimage,
    /// Debian package.
    Deb,
    /// RPM package.
    Rpm,
    /// macOS disk image.
    Dmg,
}

impl AssetKind {
    /// Structural identifier used in the response.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Installer => "installer",
            Self::Appimage => "appimage",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Dmg => "dmg",
        }
    }
}

/// One link offered by the download page.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopAsset {
    /// `windows`, `linux` or `macos`.
    pub platform: &'static str,
    /// `installer`, `appimage`, `deb`, `rpm` or `dmg`.
    pub kind: &'static str,
    /// Release asset file name.
    pub name: String,
    /// Absolute https URL, or the configured mirror URL.
    pub url: String,
    /// Size in bytes, `0` when the upstream did not say.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256, when the release publishes `SHA256SUMS`.
    pub sha256: Option<String>,
}

/// `GET /api/desktop/download` response.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopDownload {
    /// Always `stable`; early-access builds follow `/releases/tag/edge`.
    pub channel: &'static str,
    /// `github`, `mirror`, `cache` (served from the in-process cache) or
    /// `fallback` (no cached answer and the refresh failed).
    pub source: &'static str,
    /// True when the answer could not be refreshed and may be outdated.
    pub stale: bool,
    /// Release version without the leading `v`, `null` before the first
    /// successful fetch.
    pub version: Option<String>,
    /// Git tag, e.g. `v0.3.2`.
    pub tag: Option<String>,
    /// Publication date, RFC 3339.
    pub released_at: Option<String>,
    /// Human release page; always present, even when everything else failed.
    pub release_url: String,
    /// First paragraph of the release notes, plain text, bounded.
    pub notes: Option<String>,
    /// `SHA256SUMS` asset when the release publishes one.
    pub checksums_url: Option<String>,
    /// Requested platform, echoed back; `null` when the caller asked for all.
    pub platform: Option<&'static str>,
    /// Asset matching [`Self::platform`], when one was found.
    pub recommended: Option<DesktopAsset>,
    /// Every installer offered by the release.
    pub assets: Vec<DesktopAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    browser_download_url: String,
}

/// Parsed upstream release, cached as-is.
#[derive(Debug, Clone)]
struct ReleaseSnapshot {
    version: Option<String>,
    tag: Option<String>,
    released_at: Option<String>,
    release_url: String,
    notes: Option<String>,
    checksums_url: Option<String>,
    assets: Vec<DesktopAsset>,
}

impl ReleaseSnapshot {
    /// Fallback document used when no release was ever fetched.
    fn fallback(config: &Config) -> Self {
        Self {
            version: None,
            tag: None,
            released_at: None,
            release_url: safe_url(&config.desktop_releases_page)
                .unwrap_or_else(|| DEFAULT_DESKTOP_RELEASES_PAGE.to_string()),
            notes: None,
            checksums_url: None,
            assets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    fetched_at: Instant,
    release: ReleaseSnapshot,
}

fn cache() -> &'static Mutex<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(config: &Config) -> String {
    format!(
        "{}#{}",
        config.desktop_releases_api,
        config.desktop_download_ttl.as_millis()
    )
}

fn cached(key: &str) -> Option<CacheEntry> {
    cache()
        .lock()
        .map(|entries| entries.get(key).cloned())
        .unwrap_or(None)
}

fn remember(key: &str, release: &ReleaseSnapshot) {
    if let Ok(mut entries) = cache().lock() {
        entries.insert(
            key.to_string(),
            CacheEntry {
                fetched_at: Instant::now(),
                release: release.clone(),
            },
        );
    }
}

/// Drops the process-wide cache. Only used by tests.
pub fn clear_cache_for_tests() {
    if let Ok(mut entries) = cache().lock() {
        entries.clear();
    }
}

/// Resolves the release document and builds the public response.
///
/// The cache is checked first. A refresh failure keeps serving the last known
/// release (`source: "cache"`, `stale: true`); with no cached copy the caller
/// gets the releases-page fallback.
pub async fn download(
    client: &reqwest::Client,
    config: &Config,
    platform: Option<Platform>,
) -> DesktopDownload {
    let key = cache_key(config);
    let cached_entry = cached(&key);
    let ttl = config.desktop_download_ttl;
    let fresh = cached_entry
        .as_ref()
        .is_some_and(|entry| ttl > Duration::ZERO && entry.fetched_at.elapsed() < ttl);

    let (release, source, stale) = if fresh {
        let entry = cached_entry.expect("fresh cache entry");
        (entry.release, "cache", false)
    } else {
        match refresh(client, config).await {
            Ok(release) => {
                remember(&key, &release);
                (release, configured_source(config), false)
            }
            Err(error) => {
                tracing::debug!(error = %error, "desktop release metadata is unavailable");
                match cached_entry {
                    Some(entry) => (entry.release, "cache", true),
                    None => (ReleaseSnapshot::fallback(config), "fallback", true),
                }
            }
        }
    };

    let assets = release
        .assets
        .iter()
        .map(|asset| with_mirror_url(config, asset))
        .collect::<Vec<_>>();
    let recommended = platform.and_then(|platform| {
        assets
            .iter()
            .filter(|asset| asset.platform == platform.as_str())
            .min_by_key(|asset| kind_rank(asset.kind))
            .cloned()
    });

    DesktopDownload {
        channel: "stable",
        source,
        stale,
        version: release.version,
        tag: release.tag,
        released_at: release.released_at,
        release_url: release.release_url,
        notes: release.notes,
        checksums_url: release
            .checksums_url
            .as_deref()
            .and_then(|url| with_mirror(config, url)),
        platform: platform.map(Platform::as_str),
        recommended,
        assets,
    }
}

fn configured_source(config: &Config) -> &'static str {
    if config.desktop_download_base_url.is_some() {
        "mirror"
    } else {
        "github"
    }
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "installer" | "appimage" | "dmg" => 0,
        "deb" => 1,
        "rpm" => 2,
        _ => 9,
    }
}

fn with_mirror_url(config: &Config, asset: &DesktopAsset) -> DesktopAsset {
    let mut asset = asset.clone();
    if let Some(url) = with_mirror(config, &asset.url) {
        asset.url = url;
    }
    asset
}

/// Rewrites an upstream URL through the configured flat mirror, if any.
fn with_mirror(config: &Config, url: &str) -> Option<String> {
    let Some(base) = config.desktop_download_base_url.as_deref() else {
        return safe_url(url);
    };
    let name = url.rsplit('/').next().filter(|name| !name.is_empty())?;
    safe_url(&format!("{}/{name}", base.trim_end_matches('/')))
}

/// Fetches the release document and its optional `SHA256SUMS` file.
async fn refresh(client: &reqwest::Client, config: &Config) -> Result<ReleaseSnapshot, String> {
    let release = fetch_release(client, config).await?;
    let checksums = fetch_checksums(client, config, &release).await;
    Ok(finish_snapshot(
        release,
        &config.desktop_releases_page,
        checksums,
    ))
}

async fn fetch_release(
    client: &reqwest::Client,
    config: &Config,
) -> Result<ReleaseSnapshot, String> {
    let response = client
        .get(&config.desktop_releases_api)
        .header(ACCEPT, "application/vnd.github+json")
        .timeout(config.desktop_download_timeout)
        .send()
        .await
        .map_err(|error| format!("request failed: {error}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("release API answered {status}"));
    }
    let body = bounded_body(response, MAX_RELEASE_BYTES).await?;
    let document: GhRelease =
        serde_json::from_slice(&body).map_err(|error| format!("invalid release JSON: {error}"))?;
    snapshot_from_release(document)
}

async fn bounded_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if let Some(length) = response.content_length() {
        if length > limit as u64 {
            return Err(format!("response body is larger than {limit} bytes"));
        }
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("read body: {error}"))?;
    if body.len() > limit {
        return Err(format!("response body is larger than {limit} bytes"));
    }
    Ok(body.to_vec())
}

fn snapshot_from_release(document: GhRelease) -> Result<ReleaseSnapshot, String> {
    let tag = document.tag_name.trim();
    if tag.is_empty() {
        return Err("release has no tag".to_string());
    }
    let assets = document
        .assets
        .iter()
        .filter_map(asset_from_gh)
        .collect::<Vec<_>>();
    let checksum_asset = document
        .assets
        .iter()
        .find(|asset| is_checksum_name(&asset.name))
        .map(|asset| asset.browser_download_url.clone())
        .and_then(|url| safe_url(&url));

    Ok(ReleaseSnapshot {
        version: Some(tag.trim_start_matches('v').to_string()),
        tag: Some(tag.to_string()),
        released_at: document.published_at,
        release_url: document
            .html_url
            .as_deref()
            .and_then(safe_url)
            .unwrap_or_default(),
        notes: document.body.as_deref().and_then(summarize_notes),
        checksums_url: checksum_asset,
        assets,
    })
}

/// Attaches checksums and replaces an empty release URL with the releases
/// page. Used after the checksum file has been fetched.
fn finish_snapshot(
    mut release: ReleaseSnapshot,
    release_page: &str,
    checksums: Option<HashMap<String, String>>,
) -> ReleaseSnapshot {
    if release.release_url.is_empty() {
        release.release_url =
            safe_url(release_page).unwrap_or_else(|| DEFAULT_DESKTOP_RELEASES_PAGE.to_string());
    }
    if let Some(checksums) = checksums {
        for asset in &mut release.assets {
            asset.sha256 = checksums.get(&asset.name).cloned();
        }
    }
    release
}

/// Fetches `SHA256SUMS` for a release when it publishes one.
///
/// Parsing failures are not fatal: the page then links to the checksum file
/// without showing per-asset hashes.
async fn fetch_checksums(
    client: &reqwest::Client,
    config: &Config,
    release: &ReleaseSnapshot,
) -> Option<HashMap<String, String>> {
    let url = release.checksums_url.as_deref()?;
    let response = client
        .get(url)
        .timeout(config.desktop_download_timeout)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = bounded_body(response, MAX_CHECKSUM_BYTES).await.ok()?;
    let text = String::from_utf8_lossy(&body);
    Some(parse_sha256sums(&text))
}

/// Parses `sha256sum` output (`<hex>  <name>` or `<hex> *<name>`).
#[must_use]
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut hashes = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(hash) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let name = name.trim().trim_start_matches('*').trim_start_matches("./");
        if hash.len() != 64 || !hash.chars().all(|character| character.is_ascii_hexdigit()) {
            continue;
        }
        if name.is_empty() {
            continue;
        }
        hashes.insert(name.to_string(), hash.to_ascii_lowercase());
    }
    hashes
}

/// Only the release-wide manifest counts: a per-file `*.sha256` (the server
/// tarball publishes one) says nothing about the desktop installers.
fn is_checksum_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "sha256sums" | "sha256sums.txt"
    )
}

fn asset_from_gh(asset: &GhAsset) -> Option<DesktopAsset> {
    let (platform, kind) = classify_asset(&asset.name)?;
    let url = safe_url(&asset.browser_download_url)?;
    Some(DesktopAsset {
        platform: platform.as_str(),
        kind: kind.as_str(),
        name: asset.name.clone(),
        url,
        size_bytes: asset.size,
        sha256: None,
    })
}

/// Classifies a release asset into a platform installer, or `None` for
/// updater artifacts (`*.sig`, `latest.json`, `*.tar.gz`, `*.nsis.zip`) that
/// the download page must not offer as installers.
#[must_use]
pub fn classify_asset(name: &str) -> Option<(Platform, AssetKind)> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.is_empty()
        || lower.ends_with(".sig")
        || lower == "latest.json"
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".nsis.zip")
        || lower.ends_with(".zip")
    {
        return None;
    }
    if lower.ends_with(".exe") || lower.ends_with(".msi") {
        return Some((Platform::Windows, AssetKind::Installer));
    }
    if lower.ends_with(".appimage") {
        return Some((Platform::Linux, AssetKind::Appimage));
    }
    if lower.ends_with(".deb") {
        return Some((Platform::Linux, AssetKind::Deb));
    }
    if lower.ends_with(".rpm") {
        return Some((Platform::Linux, AssetKind::Rpm));
    }
    if lower.ends_with(".dmg") {
        return Some((Platform::Macos, AssetKind::Dmg));
    }
    if lower.ends_with(".pkg") {
        return Some((Platform::Macos, AssetKind::Installer));
    }
    None
}

/// Reduces a release body to one plain-text paragraph for the page summary.
#[must_use]
pub fn summarize_notes(body: &str) -> Option<String> {
    let mut paragraph: Vec<&str> = Vec::new();
    let mut in_code = false;
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if line.is_empty() {
            if paragraph.is_empty() {
                continue;
            }
            break;
        }
        if line.starts_with('#') || line.starts_with("<!--") || line.starts_with('|') {
            continue;
        }
        let stripped = line.trim_start_matches(['-', '*', '>', ' ']);
        // GitHub appends an auto-generated compare link; it is not a summary.
        if stripped.to_ascii_lowercase().starts_with("full changelog") {
            continue;
        }
        paragraph.push(stripped.trim());
    }
    let joined = paragraph.join(" ");
    let mut text = String::with_capacity(joined.len());
    let mut chars = joined.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '[' => {
                // Markdown link: keep the label, drop the target.
                let mut label = String::new();
                let mut depth = 0usize;
                let mut closed = false;
                for next in chars.by_ref() {
                    match next {
                        '[' => {
                            depth += 1;
                            label.push(next);
                        }
                        ']' if depth == 0 => {
                            closed = true;
                            break;
                        }
                        ']' => {
                            depth -= 1;
                            label.push(next);
                        }
                        _ => label.push(next),
                    }
                }
                if closed && chars.peek() == Some(&'(') {
                    for next in chars.by_ref() {
                        if next == ')' {
                            break;
                        }
                    }
                    text.push_str(&label);
                } else {
                    text.push('[');
                    text.push_str(&label);
                }
            }
            '*' | '`' | '_' | '#' => {}
            '<' => {
                // Drop a bare HTML tag, keep comparisons such as "< 1 s".
                if chars
                    .peek()
                    .is_some_and(|next| next.is_ascii_alphabetic() || *next == '/')
                {
                    for next in chars.by_ref() {
                        if next == '>' {
                            break;
                        }
                    }
                } else {
                    text.push('<');
                }
            }
            other => text.push(other),
        }
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return None;
    }
    Some(truncate_chars(&text, MAX_SUMMARY_CHARS))
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut result = text
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    result.push('…');
    result
}

/// Accepts only absolute `http`/`https` URLs. Upstream JSON is untrusted and
/// must never put a `javascript:` or `data:` URL in an anchor.
#[must_use]
pub fn safe_url(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_aliases_resolve() {
        assert_eq!(Platform::parse("Windows"), Some(Platform::Windows));
        assert_eq!(Platform::parse("win64"), Some(Platform::Windows));
        assert_eq!(Platform::parse("linux"), Some(Platform::Linux));
        assert_eq!(Platform::parse("AppImage"), Some(Platform::Linux));
        assert_eq!(Platform::parse("darwin"), Some(Platform::Macos));
        assert_eq!(Platform::parse("plan9"), None);
        assert_eq!(Platform::parse(""), None);
    }

    #[test]
    fn assets_are_classified_and_updater_files_skipped() {
        assert_eq!(
            classify_asset("Ludex_0.3.2_x64-setup.exe"),
            Some((Platform::Windows, AssetKind::Installer))
        );
        assert_eq!(
            classify_asset("ludex_0.3.2_amd64.AppImage"),
            Some((Platform::Linux, AssetKind::Appimage))
        );
        assert_eq!(
            classify_asset("ludex_0.3.2_amd64.deb"),
            Some((Platform::Linux, AssetKind::Deb))
        );
        assert_eq!(
            classify_asset("Ludex_0.3.2_universal.dmg"),
            Some((Platform::Macos, AssetKind::Dmg))
        );
        assert_eq!(classify_asset("ludex_0.3.2_x64-setup.exe.sig"), None);
        assert_eq!(classify_asset("latest.json"), None);
        assert_eq!(classify_asset("ludex_0.3.2_amd64.AppImage.tar.gz"), None);
        assert_eq!(classify_asset("ludex_0.3.2_x64.nsis.zip"), None);
        assert_eq!(classify_asset("ludex-server-v0.3.2-x86_64.tar.gz"), None);
        assert_eq!(classify_asset("SHA256SUMS"), None);
    }

    #[test]
    fn checksum_file_parses_both_git_and_sh_formats() {
        let text = "\
# generated
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  Ludex_0.3.2_x64-setup.exe
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *ludex_0.3.2_amd64.AppImage
not-a-hash  ignored.deb
";
        let hashes = parse_sha256sums(text);
        assert_eq!(
            hashes.get("Ludex_0.3.2_x64-setup.exe").map(String::as_str),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert!(hashes.contains_key("ludex_0.3.2_amd64.AppImage"));
        assert!(!hashes.contains_key("ignored.deb"));
    }

    #[test]
    fn notes_summary_is_plain_bounded_text() {
        let body = "# Ludex 0.3.2\n\n**Cloud saves** and [controllers](https://example.com).\n\nSecond paragraph.";
        let summary = summarize_notes(body).expect("summary");
        assert_eq!(summary, "Cloud saves and controllers.");
        // An automatic release only carries the compare link: no summary.
        assert_eq!(
            summarize_notes("**Full Changelog**: https://example.com/compare/v1...v2"),
            None
        );
        let long = "a".repeat(1_000);
        assert_eq!(
            summarize_notes(&long).expect("summary").chars().count(),
            MAX_SUMMARY_CHARS
        );
    }

    #[test]
    fn safe_urls_reject_active_schemes() {
        assert_eq!(
            safe_url("https://github.com/cleeryy/ludex/releases").as_deref(),
            Some("https://github.com/cleeryy/ludex/releases")
        );
        assert_eq!(safe_url("javascript:alert(1)"), None);
        assert_eq!(safe_url("data:text/html,hi"), None);
        assert_eq!(safe_url(""), None);
    }
}
