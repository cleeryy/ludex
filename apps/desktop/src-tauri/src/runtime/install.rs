//! Verified download, extraction and removal for "Make it run" (#178).
//!
//! No function here builds a shell command: installers are spawned with a
//! fixed argument list from the catalog, archives are unpacked with path
//! guards, and every artifact's size and SHA-256 are checked before it is
//! extracted or executed.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::catalog::check_download_url;
use super::detect::HostInfo;
use super::{ArchiveKind, InstallKind, Platform, RuntimeInstallRow, RuntimeProvider};
use crate::elevate;
use crate::paths;

/// Hard cap on an imported user file (GPTK DMG). Ludex never copies it.
pub const MAX_IMPORT_BYTES: u64 = 20 * 1024 * 1024 * 1024;
/// Cap on the unpacked size of one runtime artifact.
const MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_EXTRACTED_FILES: usize = 200_000;
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Runtime downloads are disposable; keep the cache bounded even across
/// pinned-version changes.
const CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    Downloading,
    Verifying,
    Extracting,
    Installing,
    Done,
    Failed,
    Cancelled,
}

impl InstallPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallPhase::Downloading => "downloading",
            InstallPhase::Verifying => "verifying",
            InstallPhase::Extracting => "extracting",
            InstallPhase::Installing => "installing",
            InstallPhase::Done => "done",
            InstallPhase::Failed => "failed",
            InstallPhase::Cancelled => "cancelled",
        }
    }
}

/// One verified artifact Ludex created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledProvider {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed(InstalledProvider),
    /// The installer was verified and downloaded but needs an elevated app.
    NeedsElevation {
        program: String,
        args: Vec<String>,
        log: String,
    },
}

/// Build the HTTP client used for runtime downloads. Redirects are re-checked
/// against the same allow-list, so a compromised release URL cannot bounce the
/// download to an arbitrary host.
pub fn runtime_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(format!("ludex-desktop/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(60 * 60))
        .redirect(reqwest::redirect::Policy::custom(
            |attempt| match check_download_url(attempt.url().as_str()) {
                Ok(()) => attempt.follow(),
                Err(_) => attempt.error("runtime redirect left the allow-list"),
            },
        ))
        .build()
        .map_err(|error| format!("Could not create the runtime download client: {error}"))
}

/// Download, verify, then unpack or run one provider.
pub async fn install_provider<F>(
    client: &reqwest::Client,
    host: &HostInfo,
    provider: &RuntimeProvider,
    cancel: &CancellationToken,
    mut progress: F,
) -> Result<InstallOutcome, String>
where
    F: FnMut(InstallPhase, u64, u64, &str) + Send,
{
    if provider.platform != host.platform {
        return Err(format!(
            "{} targets {}, not {}.",
            provider.name,
            provider.platform.as_str(),
            host.platform.as_str()
        ));
    }
    let download = provider
        .download
        .as_ref()
        .ok_or_else(|| format!("{} has no download in the manifest.", provider.name))?;
    let install = provider
        .install
        .as_ref()
        .ok_or_else(|| format!("{} has no install steps in the manifest.", provider.name))?;
    check_download_url(&download.url)?;
    if download.size_bytes == 0 || download.size_bytes > super::MAX_DOWNLOAD_BYTES {
        return Err(format!(
            "{} has an invalid download size in the manifest.",
            provider.name
        ));
    }
    if cancel.is_cancelled() {
        return Err("cancelled".to_string());
    }

    let archive = cached_artifact(host, provider);
    if !archive.is_file() {
        download_artifact(client, provider, host, cancel, &mut progress).await?;
    }
    progress(
        InstallPhase::Verifying,
        0,
        0,
        "Checking the pinned size and SHA-256",
    );
    if let Err(error) = verify_artifact(&archive, provider) {
        let _ = tokio::fs::remove_file(&archive).await;
        return Err(error);
    }
    if cancel.is_cancelled() {
        return Err("cancelled".to_string());
    }

    match install.kind {
        InstallKind::Extract => {
            progress(
                InstallPhase::Extracting,
                0,
                0,
                "Unpacking the verified archive",
            );
            let provider_clone = provider.clone();
            let host_clone = host.clone();
            let archive_clone = archive.clone();
            let installed = tokio::task::spawn_blocking(move || {
                install_extracted(&provider_clone, &host_clone, &archive_clone)
            })
            .await
            .map_err(|error| format!("The runtime extraction task failed: {error}"))??;
            prune_cache(host, Some(&archive));
            progress(InstallPhase::Done, 0, 0, "Runtime installed");
            Ok(InstallOutcome::Installed(installed))
        }
        InstallKind::Installer | InstallKind::DirectXRedist => {
            let program = archive.to_string_lossy().to_string();
            if provider.requires_admin && !host.elevated {
                let log = format!("{} {}", program, install.args.join(" "));
                return Ok(InstallOutcome::NeedsElevation {
                    program,
                    args: install.args.clone(),
                    log,
                });
            }
            progress(
                InstallPhase::Installing,
                0,
                0,
                "Running the signed installer",
            );
            let provider_clone = provider.clone();
            let archive_clone = archive.clone();
            let cancel_clone = cancel.clone();
            let host_clone = host.clone();
            let code = tokio::task::spawn_blocking(move || {
                run_installer(&provider_clone, &archive_clone, &cancel_clone, &host_clone)
            })
            .await
            .map_err(|error| format!("The installer task failed: {error}"))??;
            progress(
                InstallPhase::Done,
                0,
                0,
                &format!("Installer exited with code {code}"),
            );
            prune_cache(host, Some(&archive));
            Ok(InstallOutcome::Installed(InstalledProvider {
                path: String::new(),
                sha256: provider
                    .download
                    .as_ref()
                    .map(|download| download.sha256.clone())
                    .unwrap_or_default(),
                size_bytes: provider
                    .download
                    .as_ref()
                    .map(|download| download.size_bytes)
                    .unwrap_or(0),
                detail: format!("installer exit code {code}"),
            }))
        }
        InstallKind::Import => Err(format!(
            "{} must be imported from a file, not downloaded.",
            provider.name
        )),
    }
}

/// Path of the cached artifact. It carries the first digest bytes so two
/// catalog versions cannot share a file.
pub fn cached_artifact(host: &HostInfo, provider: &RuntimeProvider) -> PathBuf {
    let download = provider.download.as_ref();
    let digest = download
        .map(|download| download.sha256.chars().take(12).collect::<String>())
        .unwrap_or_default();
    let name = download
        .and_then(|download| {
            reqwest::Url::parse(&download.url).ok().and_then(|url| {
                url.path_segments()
                    .and_then(|mut segments| segments.next_back())
                    .map(str::to_string)
            })
        })
        .map(|name| paths::sanitize_component(&name, "runtime.bin"))
        .unwrap_or_else(|| "runtime.bin".to_string());
    host.cache_dir
        .join(format!("{}-{}-{}", provider.id, digest, name))
}

/// Elevation roots for a machine-wide runtime step (#192). Only an artifact
/// from Ludex's cache — whose size and SHA-256 matched the catalog before it
/// was written — may be executed elevated; the Program Files roots still gate
/// the service/driver and config helpers.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn elevation_roots(host: &HostInfo) -> elevate::ElevationRoots {
    elevate::ElevationRoots::from_env().with_installers(vec![host.cache_dir.clone()])
}

/// Download to a `.part` file, hashing while writing, then atomically rename.
pub async fn download_artifact<F>(
    client: &reqwest::Client,
    provider: &RuntimeProvider,
    host: &HostInfo,
    cancel: &CancellationToken,
    progress: &mut F,
) -> Result<PathBuf, String>
where
    F: FnMut(InstallPhase, u64, u64, &str) + Send,
{
    let download = provider
        .download
        .as_ref()
        .ok_or_else(|| format!("{} has no download in the manifest.", provider.name))?;
    check_download_url(&download.url)?;
    tokio::fs::create_dir_all(&host.cache_dir)
        .await
        .map_err(|error| {
            format!(
                "Could not create the runtime cache {}: {error}",
                host.cache_dir.display()
            )
        })?;
    let destination = cached_artifact(host, provider);
    let part = destination.with_extension("part");
    let _ = tokio::fs::remove_file(&part).await;

    let response = client
        .get(&download.url)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| format!("Could not download {}: {error}", provider.name))?;
    if !response.status().is_success() {
        return Err(format!(
            "{} returned HTTP {}.",
            provider.name,
            response.status()
        ));
    }
    if let Some(length) = response.content_length() {
        if length > download.size_bytes {
            return Err(format!(
                "{} is larger than the pinned size ({} > {}).",
                provider.name, length, download.size_bytes
            ));
        }
    }

    let mut file = tokio::fs::File::create(&part)
        .await
        .map_err(|error| format!("Could not create {}: {error}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut last = Instant::now();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err("cancelled".to_string());
        }
        let chunk = chunk.map_err(|error| format!("The download failed: {error}"))?;
        done = done.saturating_add(chunk.len() as u64);
        if done > download.size_bytes {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!(
                "{} sent more bytes than the pinned size.",
                provider.name
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Could not write {}: {error}", part.display()))?;
        if last.elapsed() >= Duration::from_millis(200) {
            last = Instant::now();
            progress(
                InstallPhase::Downloading,
                done,
                download.size_bytes,
                &format!("Downloading {}", provider.name),
            );
        }
    }
    file.flush()
        .await
        .map_err(|error| format!("Could not flush {}: {error}", part.display()))?;
    drop(file);

    let digest = hex::encode(hasher.finalize());
    if done != download.size_bytes || digest != download.sha256 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(format!(
            "Checksum mismatch for {}: expected {} bytes / {}, got {} bytes / {}. Nothing was extracted or executed.",
            provider.name, download.size_bytes, download.sha256, done, digest
        ));
    }
    tokio::fs::rename(&part, &destination)
        .await
        .map_err(|error| format!("Could not finalize {}: {error}", destination.display()))?;
    progress(
        InstallPhase::Downloading,
        done,
        download.size_bytes,
        &format!("{} downloaded and verified", provider.name),
    );
    Ok(destination)
}

/// Independent verification, used again right before install and by tests.
pub fn verify_artifact(path: &Path, provider: &RuntimeProvider) -> Result<(), String> {
    let download = provider
        .download
        .as_ref()
        .ok_or_else(|| format!("{} has no download in the manifest.", provider.name))?;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if metadata.len() != download.size_bytes {
        return Err(format!(
            "Size mismatch for {}: expected {} bytes, got {}.",
            provider.name,
            download.size_bytes,
            metadata.len()
        ));
    }
    let (size, digest) = hash_file(path)?;
    if size != download.size_bytes || digest != download.sha256 {
        return Err(format!(
            "Checksum mismatch for {}: expected {}, got {}. Nothing was extracted or executed.",
            provider.name, download.sha256, digest
        ));
    }
    Ok(())
}

pub fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    Ok((size, hex::encode(hasher.finalize())))
}

/// Unpack a verified archive into the managed folder. Idempotent: an existing
/// folder with the expected entry point is reused.
pub fn install_extracted(
    provider: &RuntimeProvider,
    host: &HostInfo,
    archive: &Path,
) -> Result<InstalledProvider, String> {
    let download = provider
        .download
        .as_ref()
        .ok_or_else(|| format!("{} has no download in the manifest.", provider.name))?;
    let install = provider
        .install
        .as_ref()
        .ok_or_else(|| format!("{} has no install spec in the manifest.", provider.name))?;
    let target = expand_target(&install.target, host);
    let entry = download.entry_point.as_deref();
    if let Some(entry) = entry {
        if target.join(entry).is_file() {
            return Ok(InstalledProvider {
                path: target.to_string_lossy().to_string(),
                sha256: download.sha256.clone(),
                size_bytes: download.size_bytes,
                detail: "already installed".to_string(),
            });
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    let staging = target.with_file_name(format!(
        "{}.tmp-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("runtime"),
        crate::util::short_id(&uuid::Uuid::new_v4().to_string())
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| format!("Could not create {}: {error}", staging.display()))?;

    let result = match download.archive {
        ArchiveKind::TarGz | ArchiveKind::Tar => extract_tar(
            archive,
            &staging,
            download.archive == ArchiveKind::TarGz,
            download.strip_components,
        ),
        ArchiveKind::Zip => extract_zip(archive, &staging, download.strip_components),
        ArchiveKind::Exe | ArchiveKind::Dmg => {
            Err(format!("{} is not an extractable archive.", provider.name))
        }
    };
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error);
    }

    if let Some(entry) = entry {
        let entry_path = staging.join(entry);
        if !entry_path.is_file() {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!(
                "The verified archive for {} did not contain {}.",
                provider.name, entry
            ));
        }
        make_executable(&entry_path);
    }

    if target.exists() {
        // A broken folder (entry point missing) is replaced, not left behind.
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::remove_dir_all(&target)
            .map_err(|error| format!("Could not replace {}: {error}", target.display()))?;
    }
    std::fs::rename(&staging, &target).map_err(|error| {
        let _ = std::fs::remove_dir_all(&staging);
        format!(
            "Could not move the runtime into {}: {error}",
            target.display()
        )
    })?;
    let (size, files) = paths::dir_stats(&target);
    Ok(InstalledProvider {
        path: target.to_string_lossy().to_string(),
        sha256: download.sha256.clone(),
        size_bytes: size,
        detail: format!("extracted {files} files"),
    })
}

fn expand_target(template: &str, host: &HostInfo) -> PathBuf {
    let value = template
        .replace("{appData}", &host.app_data.to_string_lossy())
        .replace("{cache}", &host.cache_dir.to_string_lossy());
    PathBuf::from(value)
}

/// Reject absolute paths, `..`, backslashes (Windows separators in a Unix
/// archive) and drive prefixes. Returns the stripped relative path.
pub fn safe_archive_path(raw: &Path, strip_components: usize) -> Result<Option<PathBuf>, String> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in raw.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str().ok_or_else(|| {
                    format!(
                        "Unsupported non-UTF-8 path in the archive: {}",
                        raw.display()
                    )
                })?;
                if text.is_empty()
                    || text.contains('\\')
                    || text.contains(':')
                    || text == "."
                    || text == ".."
                {
                    return Err(format!(
                        "The runtime archive contains an unsafe path: {}",
                        raw.display()
                    ));
                }
                parts.push(part);
            }
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "The runtime archive contains an unsafe path: {}",
                    raw.display()
                ))
            }
        }
    }
    if parts.len() < strip_components {
        return Ok(None);
    }
    let stripped = &parts[strip_components..];
    if stripped.is_empty() {
        return Ok(None);
    }
    let mut result = PathBuf::new();
    for part in stripped {
        result.push(part);
    }
    Ok(Some(result))
}

/// A symlink target must stay inside the extraction root. `..` is allowed as
/// long as it never walks above the root (GE-Proton links `../lib/...`).
pub fn safe_symlink_target(link_path: &Path, target: &Path) -> Result<PathBuf, String> {
    if target.is_absolute() {
        return Err(format!(
            "The runtime archive contains an absolute symlink: {} -> {}",
            link_path.display(),
            target.display()
        ));
    }
    let mut stack: Vec<PathBuf> = link_path
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => Some(PathBuf::from(part)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for component in target.components() {
        match component {
            Component::Normal(part) => stack.push(PathBuf::from(part)),
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return Err(format!(
                        "The runtime archive contains a symlink that escapes the folder: {} -> {}",
                        link_path.display(),
                        target.display()
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "The runtime archive contains an unsafe symlink: {} -> {}",
                    link_path.display(),
                    target.display()
                ))
            }
        }
    }
    let resolved = stack.into_iter().fold(PathBuf::new(), |mut path, part| {
        path.push(part);
        path
    });
    Ok(resolved)
}

fn extract_tar(
    archive: &Path,
    staging: &Path,
    gzip: bool,
    strip_components: usize,
) -> Result<(), String> {
    let file = std::fs::File::open(archive)
        .map_err(|error| format!("Could not open {}: {error}", archive.display()))?;
    let reader: Box<dyn Read> = if gzip {
        Box::new(flate2::read::GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut tar = tar::Archive::new(reader);
    let mut total = 0u64;
    let mut count = 0usize;
    let entries = tar
        .entries()
        .map_err(|error| format!("The runtime archive is not a valid tar: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("Invalid tar entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("Invalid tar path: {error}"))?
            .to_path_buf();
        let entry_type = entry.header().entry_type();
        let relative = match safe_archive_path(&path, strip_components)? {
            Some(relative) => relative,
            None => continue,
        };
        if entry_type.is_dir() {
            std::fs::create_dir_all(staging.join(&relative))
                .map_err(|error| format!("Could not create an archive folder: {error}"))?;
            continue;
        }
        if entry_type.is_symlink() {
            let link = entry
                .link_name()
                .map_err(|error| format!("Invalid symlink in the runtime archive: {error}"))?
                .ok_or_else(|| {
                    format!(
                        "The runtime archive contains a symlink without a target: {}",
                        path.display()
                    )
                })?;
            safe_symlink_target(&relative, &link)?;
            let target = staging.join(&relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("Could not create an archive folder: {error}"))?;
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(link.as_ref(), &target).map_err(|error| {
                format!("Could not create the symlink {}: {error}", target.display())
            })?;
            #[cfg(not(unix))]
            return Err(format!(
                "The runtime archive contains a symlink that this platform cannot create: {}",
                path.display()
            ));
            continue;
        }
        if !entry_type.is_file() {
            return Err(format!(
                "The runtime archive contains an unsupported special file: {}",
                path.display()
            ));
        }
        count += 1;
        if count > MAX_EXTRACTED_FILES {
            return Err("The runtime archive contains too many files.".to_string());
        }
        let size = entry.header().size().unwrap_or(0);
        total = total.saturating_add(size);
        if total > MAX_EXTRACTED_BYTES {
            return Err("The runtime archive is larger than the unpacked-size limit.".to_string());
        }
        let target = staging.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create an archive folder: {error}"))?;
        }
        let mut file = std::fs::File::create(&target)
            .map_err(|error| format!("Could not create {}: {error}", target.display()))?;
        std::io::copy(&mut entry, &mut file)
            .map_err(|error| format!("Could not unpack {}: {error}", target.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
            let mode = if mode == 0 { 0o644 } else { mode };
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

fn extract_zip(archive: &Path, staging: &Path, strip_components: usize) -> Result<(), String> {
    let file = std::fs::File::open(archive)
        .map_err(|error| format!("Could not open {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("The runtime archive is not a valid ZIP: {error}"))?;
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("Invalid ZIP entry: {error}"))?;
        let Some(raw) = entry.enclosed_name() else {
            return Err("The runtime archive contains an unsafe ZIP path.".to_string());
        };
        let relative = match safe_archive_path(&raw, strip_components)? {
            Some(relative) => relative,
            None => continue,
        };
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(format!(
                    "The runtime archive contains a symlink: {}",
                    raw.display()
                ));
            }
        }
        if entry.is_dir() {
            std::fs::create_dir_all(staging.join(&relative))
                .map_err(|error| format!("Could not create an archive folder: {error}"))?;
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_EXTRACTED_BYTES {
            return Err("The runtime archive is larger than the unpacked-size limit.".to_string());
        }
        let target = staging.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create an archive folder: {error}"))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|error| format!("Could not create {}: {error}", target.display()))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|error| format!("Could not unpack {}: {error}", target.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.unix_mode().unwrap_or(0o644) & 0o777;
            let mode = if mode == 0 { 0o644 } else { mode };
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Run a verified installer with fixed arguments. Exit code 3010 means
/// "reboot required" and is a success.
fn run_installer(
    provider: &RuntimeProvider,
    archive: &Path,
    cancel: &CancellationToken,
    host: &HostInfo,
) -> Result<i32, String> {
    let install = provider
        .install
        .as_ref()
        .ok_or_else(|| format!("{} has no install spec.", provider.name))?;
    if provider.platform != Platform::Windows || !cfg!(target_os = "windows") {
        return Err(format!(
            "{} can only run on Windows; this machine is {}.",
            provider.name,
            Platform::current().as_str()
        ));
    }
    if install.kind == InstallKind::DirectXRedist {
        let extract_dir = expand_target(&install.target, host);
        let _ = std::fs::remove_dir_all(&extract_dir);
        std::fs::create_dir_all(&extract_dir)
            .map_err(|error| format!("Could not create {}: {error}", extract_dir.display()))?;
        let args: Vec<String> = install
            .args
            .iter()
            .map(|arg| arg.replace("{extract}", &extract_dir.to_string_lossy()))
            .collect();
        run_command(archive, &args, cancel)?;
        let setup = find_file(&extract_dir, "DXSETUP.exe", 3).ok_or_else(|| {
            "The DirectX redistributable did not contain DXSETUP.exe.".to_string()
        })?;
        return run_command(&setup, &install.inner_args, cancel);
    }
    run_command(archive, &install.args, cancel)
}

fn run_command(program: &Path, args: &[String], cancel: &CancellationToken) -> Result<i32, String> {
    use std::process::Command;
    let mut child = Command::new(program)
        .args(args)
        .spawn()
        .map_err(|error| format!("Could not run {}: {error}", program.display()))?;
    let started = Instant::now();
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".to_string());
        }
        if started.elapsed() > INSTALL_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{} did not finish within 30 minutes.",
                program.display()
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(-1);
                if code == 0 || code == 3010 {
                    return Ok(code);
                }
                return Err(format!(
                    "{} exited with code {code}. Run it manually if it needs elevation.",
                    program.display()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(error) => return Err(format!("Could not wait for {}: {error}", program.display())),
        }
    }
}

fn find_file(root: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_file(&path, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Record a user-supplied GPTK DMG. The file is hashed in place; it is never
/// copied, moved or bundled.
pub fn import_gptk(provider: &RuntimeProvider, source: &Path) -> Result<InstalledProvider, String> {
    let install = provider
        .install
        .as_ref()
        .ok_or_else(|| format!("{} has no import spec.", provider.name))?;
    if install.kind != InstallKind::Import {
        return Err(format!("{} is not an import-only provider.", provider.name));
    }
    let canonical = source
        .canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", source.display()))?;
    let metadata = std::fs::metadata(&canonical)
        .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
    if !metadata.is_file() {
        return Err("The imported runtime must be a file.".to_string());
    }
    if canonical
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("dmg"))
        != Some(true)
    {
        return Err("Game Porting Toolkit is distributed as a .dmg file.".to_string());
    }
    if metadata.len() == 0 || metadata.len() > MAX_IMPORT_BYTES {
        return Err(
            "The selected DMG is empty or larger than the 20 GiB import limit.".to_string(),
        );
    }
    let (size, sha256) = hash_file(&canonical)?;
    Ok(InstalledProvider {
        path: canonical.to_string_lossy().to_string(),
        sha256,
        size_bytes: size,
        detail: "imported DMG; evaluation licence shown to the user, nothing copied".to_string(),
    })
}

/// Create the per-game prefix and a marker the launch path can audit.
pub fn prepare_prefix(
    prefix: &Path,
    provider_id: &str,
    version: &str,
    now: &str,
) -> Result<u64, String> {
    if prefix.exists() && !prefix.is_dir() {
        return Err(format!("{} exists and is not a folder.", prefix.display()));
    }
    std::fs::create_dir_all(prefix)
        .map_err(|error| format!("Could not create {}: {error}", prefix.display()))?;
    let marker = prefix.join(".ludex-runtime.json");
    if !marker.exists() {
        let body = serde_json::json!({
            "managedBy": "ludex",
            "providerId": provider_id,
            "version": version,
            "createdAt": now,
        });
        std::fs::write(
            &marker,
            serde_json::to_vec_pretty(&body).unwrap_or_default(),
        )
        .map_err(|error| format!("Could not write {}: {error}", marker.display()))?;
    }
    Ok(paths::dir_size(prefix))
}

/// Keep the download cache under [`CACHE_MAX_BYTES`]. Old artifacts are
/// deleted first; the file just used is kept so a retry does not re-download.
pub fn prune_cache(host: &HostInfo, keep: Option<&Path>) -> u64 {
    let Ok(entries) = std::fs::read_dir(&host.cache_dir) else {
        return 0;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata.modified().ok()?;
            Some((path, metadata.len(), modified))
        })
        .filter(|(path, _, _)| keep.map(|keep| keep != path.as_path()).unwrap_or(true))
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    if total <= CACHE_MAX_BYTES {
        return 0;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    let mut freed = 0u64;
    for (path, size, _) in files {
        if total <= CACHE_MAX_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
            freed = freed.saturating_add(size);
        }
    }
    freed
}

/// Delete files Ludex created for a provider. An imported file is only
/// forgotten, never deleted.
pub fn remove_provider(
    host: &HostInfo,
    provider: &RuntimeProvider,
    record: &RuntimeInstallRow,
) -> Result<u64, String> {
    let imported = provider
        .install
        .as_ref()
        .map(|install| install.kind == InstallKind::Import)
        .unwrap_or(false);
    if imported {
        return Ok(0);
    }
    // Registry redistributables have no folder Ludex owns; the record is the
    // only thing to forget.
    if record.path.trim().is_empty() {
        return Ok(0);
    }
    // The cached archive is also Ludex-created and can be large (Proton).
    let cache = cached_artifact(host, provider);
    let _ = std::fs::remove_file(cache);
    let path = PathBuf::from(&record.path);
    if !path.exists() {
        prune_cache(host, None);
        return Ok(0);
    }
    let root = host
        .app_data
        .join("runtimes")
        .canonicalize()
        .map_err(|error| format!("Could not resolve the runtime folder: {error}"))?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", path.display()))?;
    if canonical == root || !canonical.starts_with(&root) {
        return Err(format!(
            "Refusing to delete {}: it is outside the Ludex runtime folder.",
            canonical.display()
        ));
    }
    let size = paths::dir_size(&canonical);
    std::fs::remove_dir_all(&canonical)
        .map_err(|error| format!("Could not delete {}: {error}", canonical.display()))?;
    prune_cache(host, None);
    Ok(size)
}

/// `{appData}/runtime-cache/logs/<game>-<timestamp>.log`.
pub fn log_path(host: &HostInfo, game_id: i64, now: &str) -> PathBuf {
    let stamp = now.replace([':', '.'], "-");
    host.cache_dir
        .join("logs")
        .join(format!("game-{game_id}-{stamp}.log"))
}

/// Append one line to the apply log. Never fails the run.
pub fn append_log(path: &Path, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{catalog, DownloadArtifact};

    fn provider() -> RuntimeProvider {
        catalog::build()
            .providers
            .into_iter()
            .find(|provider| provider.id == "umu-launcher")
            .unwrap()
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-runtime-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn host(app_data: &Path) -> HostInfo {
        HostInfo {
            platform: Platform::current(),
            arch: "x86_64".to_string(),
            os_version: None,
            elevated: false,
            home: None,
            app_data: app_data.to_path_buf(),
            cache_dir: app_data.join("runtime-cache"),
            system_root: PathBuf::from("C:\\Windows"),
            free_bytes: None,
        }
    }

    #[test]
    fn archive_paths_reject_traversal_and_strip_components() {
        assert_eq!(
            safe_archive_path(Path::new("GE-Proton11-12/proton"), 1).unwrap(),
            Some(PathBuf::from("proton"))
        );
        assert_eq!(
            safe_archive_path(Path::new("umu/umu-run"), 0).unwrap(),
            Some(PathBuf::from("umu/umu-run"))
        );
        for bad in ["../evil", "/etc/passwd", "a\\..\\b", "C:evil", "a/../../b"] {
            assert!(
                safe_archive_path(Path::new(bad), 0).is_err(),
                "{bad} must be rejected"
            );
        }
        assert_eq!(safe_archive_path(Path::new("top"), 1).unwrap(), None);
    }

    #[test]
    fn checksum_verification_aborts_before_install() {
        let dir = temp_dir("checksum");
        let file = dir.join("artifact.bin");
        std::fs::write(&file, b"not the pinned bytes").unwrap();
        let provider = provider();
        let error = verify_artifact(&file, &provider).unwrap_err();
        assert!(error.contains("Size mismatch"), "{error}");

        // Same size, wrong digest: the checksum, not only the size, must fail.
        let mut provider = provider;
        let download = provider.download.as_mut().unwrap();
        let size = download.size_bytes;
        std::fs::write(&file, vec![0u8; size as usize]).unwrap();
        let error = verify_artifact(&file, &provider).unwrap_err();
        assert!(error.contains("Checksum mismatch"), "{error}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_matching_file_verifies_and_hashes() {
        let dir = temp_dir("hash");
        let file = dir.join("artifact.bin");
        std::fs::write(&file, b"hello").unwrap();
        let (size, digest) = hash_file(&file).unwrap();
        assert_eq!(size, 5);
        assert_eq!(
            digest,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn internal_symlinks_are_kept_and_escaping_ones_are_rejected() {
        assert_eq!(
            safe_symlink_target(Path::new("files/lib/libfoo.so"), Path::new("libfoo.so.1"))
                .unwrap(),
            PathBuf::from("files/lib/libfoo.so.1")
        );
        assert_eq!(
            safe_symlink_target(Path::new("files/lib/libfoo.so"), Path::new("../share/x")).unwrap(),
            PathBuf::from("files/share/x")
        );
        for bad in ["../../../etc/passwd", "/etc/passwd"] {
            assert!(
                safe_symlink_target(Path::new("files/lib/libfoo.so"), Path::new(bad)).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn extraction_is_traversal_safe_and_idempotent() {
        let dir = temp_dir("extract");
        let app_data = dir.join("app");
        std::fs::create_dir_all(&app_data).unwrap();
        let archive_path = dir.join("umu.tar");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut builder = tar::Builder::new(file);
            let mut header = tar::Header::new_gnu();
            let payload = b"#!/bin/sh\n";
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "umu/umu-run", &payload[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut provider = provider();
        provider.download.as_mut().unwrap().sha256 = "0".repeat(64);
        provider.download.as_mut().unwrap().size_bytes = 1;
        provider.install.as_mut().unwrap().target =
            "{appData}/runtimes/umu-launcher/1.4.4".to_string();
        // install_extracted does not re-check the digest; verify_artifact does.
        let host = host(&app_data);
        let installed = install_extracted(&provider, &host, &archive_path).unwrap();
        assert!(Path::new(&installed.path).join("umu/umu-run").is_file());
        // A second call reuses the folder instead of duplicating it.
        let again = install_extracted(&provider, &host, &archive_path).unwrap();
        assert_eq!(again.path, installed.path);
        assert_eq!(again.detail, "already installed");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn tar_symlinks_are_rejected() {
        let dir = temp_dir("symlink");
        let archive_path = dir.join("bad.tar");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut builder = tar::Builder::new(file);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_cksum();
            builder
                .append_link(&mut header, "link", "/etc/passwd")
                .unwrap();
            builder.finish().unwrap();
        }
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        let error = extract_tar(&archive_path, &staging, false, 0).unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn tar_internal_symlinks_extract_safely() {
        let dir = temp_dir("symlink-ok");
        let archive_path = dir.join("ok.tar");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut builder = tar::Builder::new(file);
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "files/lib/libfoo.so.1", &b"lib\0"[..])
                .unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_cksum();
            builder
                .append_link(&mut link, "files/lib/libfoo.so", "libfoo.so.1")
                .unwrap();
            builder.finish().unwrap();
        }
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        extract_tar(&archive_path, &staging, false, 0).unwrap();
        let link = staging.join("files/lib/libfoo.so");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            PathBuf::from("libfoo.so.1")
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn import_requires_a_dmg_and_hashes_it_in_place() {
        let dir = temp_dir("import");
        let dmg = dir.join("gptk.dmg");
        std::fs::write(&dmg, b"dmg-contents").unwrap();
        let provider = catalog::build()
            .providers
            .into_iter()
            .find(|provider| provider.id == "gptk")
            .unwrap();
        let imported = import_gptk(&provider, &dmg).unwrap();
        assert_eq!(imported.size_bytes, 12);
        assert!(
            Path::new(&imported.path).is_file(),
            "the DMG is not copied or moved"
        );
        std::fs::remove_dir_all(dir).ok();

        let dir = temp_dir("import-bad");
        let txt = dir.join("not-a-dmg.txt");
        std::fs::write(&txt, b"x").unwrap();
        let error = import_gptk(&provider, &txt).unwrap_err();
        assert!(error.contains(".dmg"), "{error}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn removal_refuses_paths_outside_the_runtime_folder() {
        let dir = temp_dir("remove");
        let app_data = dir.join("app");
        std::fs::create_dir_all(app_data.join("runtimes/umu-launcher/1.4.4")).unwrap();
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let host = host(&app_data);
        let provider = provider();
        let record = RuntimeInstallRow {
            provider_id: "umu-launcher".to_string(),
            platform: Platform::current().as_str().to_string(),
            version: "1.4.4".to_string(),
            path: outside.to_string_lossy().to_string(),
            sha256: String::new(),
            size_bytes: 0,
            installed_at: String::new(),
        };
        assert!(remove_provider(&host, &provider, &record).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn removal_deletes_only_the_managed_folder_and_cache() {
        let dir = temp_dir("remove-ok");
        let app_data = dir.join("app");
        let managed = app_data.join("runtimes/umu-launcher/1.4.4");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(managed.join("umu-run"), b"x").unwrap();
        std::fs::create_dir_all(app_data.join("runtime-cache")).unwrap();
        let provider = provider();
        let cache = cached_artifact(&host(&app_data), &provider);
        std::fs::write(&cache, b"cached").unwrap();

        let record = RuntimeInstallRow {
            provider_id: "umu-launcher".to_string(),
            platform: Platform::current().as_str().to_string(),
            version: "1.4.4".to_string(),
            path: managed.to_string_lossy().to_string(),
            sha256: String::new(),
            size_bytes: 0,
            installed_at: String::new(),
        };
        let freed = remove_provider(&host(&app_data), &provider, &record).unwrap();
        assert!(freed > 0);
        assert!(!managed.exists());
        assert!(!cache.exists());
        assert!(app_data.exists(), "the app data folder survives");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_registry_install_is_forgotten_without_a_folder() {
        let dir = temp_dir("forget-registry");
        let provider = catalog::build()
            .providers
            .into_iter()
            .find(|provider| provider.id == "directx-jun2010")
            .unwrap();
        let record = RuntimeInstallRow {
            provider_id: "directx-jun2010".to_string(),
            platform: "windows".to_string(),
            version: "9.29.1974".to_string(),
            path: String::new(),
            sha256: String::new(),
            size_bytes: 1,
            installed_at: String::new(),
        };
        let freed = remove_provider(&host(&dir), &provider, &record).unwrap();
        assert_eq!(freed, 0);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn imported_gptk_is_forgotten_without_deleting_the_dmg() {
        let dir = temp_dir("forget-import");
        let dmg = dir.join("gptk.dmg");
        std::fs::write(&dmg, b"x").unwrap();
        let provider = catalog::build()
            .providers
            .into_iter()
            .find(|provider| provider.id == "gptk")
            .unwrap();
        let record = RuntimeInstallRow {
            provider_id: "gptk".to_string(),
            platform: "macos".to_string(),
            version: "user-supplied".to_string(),
            path: dmg.to_string_lossy().to_string(),
            sha256: String::new(),
            size_bytes: 1,
            installed_at: String::new(),
        };
        let freed = remove_provider(&host(&dir), &provider, &record).unwrap();
        assert_eq!(freed, 0);
        assert!(dmg.exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn prepare_prefix_writes_a_marker_once() {
        let dir = temp_dir("prefix");
        let prefix = dir.join("prefix");
        prepare_prefix(&prefix, "umu-launcher", "1.4.4", "2026-11-12T00:00:00Z").unwrap();
        let marker = prefix.join(".ludex-runtime.json");
        assert!(marker.is_file());
        let body = std::fs::read_to_string(&marker).unwrap();
        assert!(body.contains("umu-launcher"));
        // A second run does not overwrite the marker.
        std::fs::write(&marker, b"custom").unwrap();
        prepare_prefix(&prefix, "ge-proton", "11-12", "later").unwrap();
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "custom");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn provider_without_a_manifest_download_is_refused() {
        let dir = temp_dir("no-download");
        let mut provider = provider();
        provider.download = None;
        let host = host(&dir);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(install_provider(
                &runtime_client().unwrap(),
                &host,
                &provider,
                &CancellationToken::new(),
                |_, _, _, _| {},
            ))
            .unwrap_err();
        assert!(error.contains("no download"), "{error}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn manifest_mismatch_is_a_check_not_a_panic() {
        let dir = temp_dir("mismatch");
        let file = dir.join("artifact.bin");
        std::fs::write(&file, b"x").unwrap();
        let mut provider = provider();
        provider.download = Some(DownloadArtifact {
            url: "https://github.com/foo/bar".to_string(),
            size_bytes: 1,
            sha256: "0".repeat(64),
            archive: ArchiveKind::Tar,
            strip_components: 0,
            entry_point: None,
        });
        assert!(verify_artifact(&file, &provider).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
