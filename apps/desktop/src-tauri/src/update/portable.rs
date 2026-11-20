//! Portable Linux self-update (#312).
//!
//! A bare ELF in `/opt/ludex` (or `~/.local/bin`) is not an AppImage and does
//! not belong to a dpkg/rpm package. The Tauri updater has no path for it: a
//! build compiled for the rpm bundle runs `rpm`, a build compiled for the deb
//! bundle runs `dpkg`, and both end in the generic "Failed to install package".
//!
//! This module owns the third path. It downloads the `.deb` the release already
//! publishes (edge and stable both ship it), verifies it with the same minisign
//! key as the updater (`plugins.updater.pubkey`), extracts `usr/bin/ludex`,
//! checks it is a 64-bit ELF, and replaces the running executable atomically:
//!
//! 1. the verified binary is staged in `{appDataDir}/updates/ludex-<version>`
//!    so a refused install can name a file the user can `sudo install`;
//! 2. a temporary file is written next to the target, fsynced and chmod 0755;
//! 3. the target is renamed to `<target>.old`, the temporary file to the
//!    target, and the directory is fsynced;
//! 4. if the second rename fails, `<target>.old` is renamed back, so a failure
//!    never leaves a missing or half-written executable.
//!
//! Nothing is replaced before the archive verified, and a non-writable folder
//! produces an error naming the path, its owner and mode, the cause, and the
//! exact `sudo install` command (see [`TargetPermission`]).
//!
//! Every step is logged with the URL (query-free), the install kind, the
//! verification result, the target and the failure cause; see
//! [`AttemptFields::summary`] and `docs/UPDATES.md`.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::app::{configured_pubkey, endpoint_for, UpdateStatus};
use super::{scrub_text, scrub_url, UpdateProgress};
use crate::install_health;
use crate::state::AppState;

/// Platform key of the Debian artifact in `latest.json`.
pub const DEB_ASSET: &str = "linux-x86_64-deb";
/// Path of the desktop executable inside the `.deb` payload.
pub const DEB_BINARY: &str = "usr/bin/ludex";
/// Folder under the app data directory that keeps the verified binary.
pub const STAGING_DIR: &str = "updates";

/// `latest.json` is a few hundred bytes; anything bigger is not a manifest.
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
/// The release `.deb` is ~37 MB; this is a hard stop, not a target.
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
/// A Ludex ELF is ~36 MB; an archive member bigger than this is not one.
const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;
/// Progress events are throttled to four per second, like the plugin path.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/* ------------------------------ manifest -------------------------------- */

/// The channel manifest the updater already reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,
    #[serde(default)]
    pub platforms: std::collections::BTreeMap<String, ManifestAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ManifestAsset {
    /// Base64 of the minisign signature, exactly like the plugin's `signature`.
    #[serde(default)]
    pub signature: String,
    pub url: String,
}

impl ReleaseManifest {
    /// Parse `latest.json`.
    pub fn parse(text: &str) -> Result<Self, PortableError> {
        serde_json::from_str(text).map_err(|error| PortableError::Manifest {
            cause: format!("the release manifest is not valid JSON ({error})"),
        })
    }

    /// Asset for a platform key, with an actionable error when it is absent.
    pub fn asset(&self, key: &str) -> Result<&ManifestAsset, PortableError> {
        self.platforms
            .get(key)
            .ok_or_else(|| PortableError::MissingAsset {
                version: self.version.clone(),
                asset: key.to_string(),
            })
    }
}

/* ------------------------------- errors --------------------------------- */

/// Why a portable update did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortableError {
    Refused { reason: String },
    Manifest { cause: String },
    MissingAsset { version: String, asset: String },
    Download { url: String, cause: String },
    Signature { cause: String },
    Archive { cause: String },
    Binary { cause: String },
    Staging { path: PathBuf, cause: String },
    NotWritable(Box<TargetPermission>),
    Replace(Box<ReplaceFailure>),
}

impl PortableError {
    /// Short stage label, used as a log field.
    #[must_use]
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Refused { .. } => "refused",
            Self::Manifest { .. } => "manifest",
            Self::MissingAsset { .. } => "manifest",
            Self::Download { .. } => "download",
            Self::Signature { .. } => "signature",
            Self::Archive { .. } => "archive",
            Self::Binary { .. } => "binary",
            Self::Staging { .. } => "staging",
            Self::NotWritable(_) => "writability",
            Self::Replace(_) => "replace",
        }
    }

    /// One sentence for the UI, precise about what happened and what to do.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Refused { reason } => format!("Could not install the update. {reason}."),
            Self::Manifest { cause } => format!(
                "Could not install the update. {cause}. Try again, then update from the release page if it keeps failing."
            ),
            Self::MissingAsset { version, asset } => format!(
                "Could not install the update. Release {version} has no `{asset}` artifact, which this portable install needs. Download it from https://github.com/cleeryy/ludex/releases and replace the binary yourself."
            ),
            Self::Download { url, cause } => format!(
                // reqwest repeats the full URL in its error text, query string
                // included; both halves are scrubbed before anyone sees them.
                "Could not install the update. The download from {} failed ({}). Check your connection and try again.",
                scrub_url(url),
                scrub_text(cause)
            ),
            Self::Signature { cause } => format!(
                "Could not install the update. The download did not match the release signature, so Ludex refused to install it ({cause}). Nothing was replaced."
            ),
            Self::Archive { cause } => format!(
                "Could not install the update. The release package could not be read ({cause}). Nothing was replaced."
            ),
            Self::Binary { cause } => format!(
                "Could not install the update. The release package did not contain a valid Linux executable ({cause}). Nothing was replaced."
            ),
            Self::Staging { path, cause } => format!(
                "Could not install the update. The verified binary could not be kept at {} ({cause}). Nothing was replaced.",
                path.display()
            ),
            Self::NotWritable(permission) => permission.message(),
            Self::Replace(failure) => failure.message(),
        }
    }
}

/// A failed replacement of the target executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceFailure {
    pub target: PathBuf,
    pub cause: String,
    pub state: ReplaceState,
}

/// What happened to the current binary when the replacement failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceState {
    /// Nothing was renamed: the current binary is the one that was running.
    Untouched,
    /// The new file could not take its place and the previous binary was
    /// renamed back.
    RolledBack,
    /// Both renames failed: the previous binary is at this path.
    BackupKept(PathBuf),
}

impl ReplaceFailure {
    #[must_use]
    pub fn message(&self) -> String {
        let target = self.target.display();
        match &self.state {
            ReplaceState::Untouched => format!(
                "Could not install the update. {target} could not be replaced ({}) and the current binary was left untouched.",
                self.cause
            ),
            ReplaceState::RolledBack => format!(
                "Could not install the update. {target} could not be replaced ({}); the previous binary was restored.",
                self.cause
            ),
            ReplaceState::BackupKept(backup) => format!(
                "Could not install the update. {target} could not be replaced ({}). The previous binary is kept at {}; move it back to {target} to restore Ludex.",
                self.cause,
                backup.display()
            ),
        }
    }
}

/// A target folder the process may not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetPermission {
    pub target: PathBuf,
    pub dir: PathBuf,
    /// `user:group`, or numeric ids when the names cannot be resolved.
    pub owner: String,
    /// Octal mode of the target (or of the folder when the target is missing).
    pub mode: String,
    pub cause: String,
    /// Verified binary kept for the manual command, when staging succeeded.
    pub staged: Option<PathBuf>,
}

impl TargetPermission {
    /// Describe a failed write probe on `dir`.
    pub fn describe(
        target: &Path,
        dir: &Path,
        cause: &std::io::Error,
        staged: Option<&Path>,
    ) -> Self {
        let (owner, mode) = metadata_label(target)
            .or_else(|| metadata_label(dir))
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));
        Self {
            target: target.to_path_buf(),
            dir: dir.to_path_buf(),
            owner,
            mode,
            cause: cause.to_string(),
            staged: staged.map(Path::to_path_buf),
        }
    }

    /// The message a user (or a maintainer reading the log) can act on.
    #[must_use]
    pub fn message(&self) -> String {
        let target = self.target.display();
        let mut message = format!(
            "Could not install the update. {target} is owned by {} (mode {}) and {} is not writable by this account: {}. Nothing was replaced.",
            self.owner,
            self.mode,
            self.dir.display(),
            self.cause
        );
        match &self.staged {
            Some(staged) => {
                message.push_str(&format!(
                    " The verified update is kept at {}; install it with:\n  sudo install -m755 '{}' '{}'\nthen start Ludex again.",
                    staged.display(),
                    staged.display(),
                    target
                ));
            }
            None => message.push_str(&format!(
                " Download the release from https://github.com/cleeryy/ludex/releases, then copy the binary to {target} as an administrator."
            )),
        }
        message
    }
}

/// `owner:group` and `0755` for a path, when the platform exposes them.
fn metadata_label(path: &Path) -> Option<(String, String)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).ok()?;
        let uid = metadata.uid();
        let gid = metadata.gid();
        let user = name_from_table(&fs::read_to_string("/etc/passwd").unwrap_or_default(), uid)
            .unwrap_or_else(|| uid.to_string());
        let group = name_from_table(&fs::read_to_string("/etc/group").unwrap_or_default(), gid)
            .unwrap_or_else(|| gid.to_string());
        Some((
            format!("{user}:{group}"),
            format!("{:04o}", metadata.mode() & 0o7777),
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Name of an id in a `passwd`-style table (the id is the third field in both
/// `/etc/passwd` and `/etc/group`).
fn name_from_table(table: &str, id: u32) -> Option<String> {
    table.lines().find_map(|line| {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let value: u32 = fields.nth(1)?.parse().ok()?;
        (value == id && !name.is_empty()).then(|| name.to_string())
    })
}

/* ---------------------------- filesystem hooks --------------------------- */

/// Filesystem primitives of the atomic replace, injectable so the failure
/// paths (read-only folder, failed rename) are testable as an unprivileged
/// user and as root.
pub trait UpdateFs {
    fn probe_writable(&self, dir: &Path) -> std::io::Result<()>;
    fn write_synced(&self, path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn sync_dir(&self, dir: &Path) -> std::io::Result<()>;
}

/// The real filesystem.
pub struct SystemFs;

impl UpdateFs for SystemFs {
    fn probe_writable(&self, dir: &Path) -> std::io::Result<()> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        let probe = dir.join(format!(".ludex-write-probe-{}-{nanos}", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        drop(file);
        let _ = fs::remove_file(&probe);
        Ok(())
    }

    fn write_synced(&self, path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.write_all(bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        fs::rename(from, to)
    }

    fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            File::open(dir)?.sync_all()
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            Ok(())
        }
    }
}

/* ------------------------------ install --------------------------------- */

/// Everything one portable install needs, resolved before any file is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortablePlan {
    pub version: String,
    pub signature: String,
    pub pubkey: String,
    pub target: PathBuf,
    pub staging: PathBuf,
}

/// A verified binary that is now the installed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: String,
    pub target: PathBuf,
    /// Previous executable, kept for rollback.
    pub backup: Option<PathBuf>,
    pub sha256: String,
    /// Build commit embedded in the new binary, when it carries one.
    pub commit: Option<String>,
    pub bytes: usize,
}

/// Checks a downloaded artifact against a release signature.
pub trait ArtifactVerifier {
    fn verify(&self, artifact: &[u8], signature: &str, pubkey: &str) -> Result<(), String>;
}

/// Minisign, with the same key and the same call as `tauri-plugin-updater`.
pub struct MinisignVerifier;

impl ArtifactVerifier for MinisignVerifier {
    fn verify(&self, artifact: &[u8], signature: &str, pubkey: &str) -> Result<(), String> {
        let key_text = base64_text(pubkey)?;
        let public_key = minisign_verify::PublicKey::decode(&key_text)
            .map_err(|error| format!("the release public key is unusable ({error})"))?;
        let signature_text = base64_text(signature)?;
        let signature = minisign_verify::Signature::decode(&signature_text)
            .map_err(|error| format!("the release signature is unusable ({error})"))?;
        public_key
            .verify(artifact, &signature, true)
            .map_err(|error| format!("the signature does not cover this download ({error})"))
    }
}

/// Decode the base64 wrapper Tauri writes around keys and signatures.
fn base64_text(value: &str) -> Result<String, String> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .map_err(|error| format!("the release metadata is not valid base64 ({error})"))?;
    String::from_utf8(decoded).map_err(|_| "the release metadata is not UTF-8".to_string())
}

/// Verify, extract, stage and replace. No network, no UI: the caller resolves
/// the artifact and this function is what the tests call (no real download).
pub fn install_verified(
    fs: &dyn UpdateFs,
    plan: &PortablePlan,
    artifact: &[u8],
    verifier: &dyn ArtifactVerifier,
) -> Result<Installed, PortableError> {
    if artifact.is_empty() {
        return Err(PortableError::Archive {
            cause: "the download is empty".to_string(),
        });
    }
    if artifact.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err(PortableError::Archive {
            cause: format!(
                "the download is {} bytes, past the {MAX_ARTIFACT_BYTES}-byte limit",
                artifact.len()
            ),
        });
    }
    if plan.signature.trim().is_empty() {
        return Err(PortableError::Signature {
            cause: "the release manifest has no signature for this artifact".to_string(),
        });
    }
    verifier
        .verify(artifact, &plan.signature, &plan.pubkey)
        .map_err(|cause| PortableError::Signature { cause })?;

    let sha256 = sha256_hex(artifact);
    let binary = extract_deb_binary(artifact, DEB_BINARY)?;
    let commit = validate_binary(&binary)?;
    write_staged(fs, &plan.staging, &binary)?;
    let replaced = replace_binary(fs, &plan.target, &binary, Some(&plan.staging))?;
    Ok(Installed {
        version: plan.version.clone(),
        target: replaced.target,
        backup: replaced.backup,
        sha256,
        commit,
        bytes: binary.len(),
    })
}

/// Result of [`replace_binary`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub target: PathBuf,
    pub backup: Option<PathBuf>,
}

/// Replace `target` with `bytes`, atomically and reversibly.
///
/// The write, the fsync and the mode are applied to a sibling file first, so a
/// crash or a full disk during the copy leaves the running binary intact. The
/// current binary is renamed to `<target>.old` before the new one takes its
/// place; a failed second rename puts it back.
pub fn replace_binary(
    fs: &dyn UpdateFs,
    target: &Path,
    bytes: &[u8],
    staged: Option<&Path>,
) -> Result<Replaced, PortableError> {
    let dir = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| PortableError::Binary {
            cause: format!("{} has no parent directory", target.display()),
        })?;
    fs.probe_writable(dir).map_err(|cause| {
        PortableError::NotWritable(Box::new(TargetPermission::describe(
            target, dir, &cause, staged,
        )))
    })?;

    let name = target
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_else(|| OsString::from("ludex"));
    let mut temp_name = name.clone();
    temp_name.push(format!(".new-{}", std::process::id()));
    let temp = dir.join(temp_name);
    let mut backup_name = name;
    backup_name.push(".old");
    let backup = dir.join(backup_name);

    if let Err(cause) = fs.write_synced(&temp, bytes, 0o755) {
        let _ = fs::remove_file(&temp);
        return Err(PortableError::Replace(Box::new(ReplaceFailure {
            target: target.to_path_buf(),
            cause: cause.to_string(),
            state: ReplaceState::Untouched,
        })));
    }

    let existed = target.exists();
    if existed {
        if let Err(cause) = fs.rename(target, &backup) {
            let _ = fs::remove_file(&temp);
            return Err(PortableError::Replace(Box::new(ReplaceFailure {
                target: target.to_path_buf(),
                cause: cause.to_string(),
                state: ReplaceState::Untouched,
            })));
        }
    }

    if let Err(cause) = fs.rename(&temp, target) {
        let restored = existed && fs.rename(&backup, target).is_ok();
        let _ = fs::remove_file(&temp);
        let state = if !existed {
            ReplaceState::Untouched
        } else if restored {
            ReplaceState::RolledBack
        } else {
            ReplaceState::BackupKept(backup.clone())
        };
        return Err(PortableError::Replace(Box::new(ReplaceFailure {
            target: target.to_path_buf(),
            cause: cause.to_string(),
            state,
        })));
    }

    let _ = fs.sync_dir(dir);
    Ok(Replaced {
        target: target.to_path_buf(),
        backup: existed.then_some(backup),
    })
}

/// Keep the verified binary under the app data directory, so a refused install
/// can print a command that works. Older staged builds are removed.
fn write_staged(fs: &dyn UpdateFs, path: &Path, bytes: &[u8]) -> Result<(), PortableError> {
    let dir = path.parent().ok_or_else(|| PortableError::Staging {
        path: path.to_path_buf(),
        cause: "the staging path has no parent".to_string(),
    })?;
    fs::create_dir_all(dir).map_err(|cause| PortableError::Staging {
        path: path.to_path_buf(),
        cause: cause.to_string(),
    })?;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let stale =
                entry.path() != path && entry.file_name().to_string_lossy().starts_with("ludex-");
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    fs.write_synced(path, bytes, 0o755)
        .map_err(|cause| PortableError::Staging {
            path: path.to_path_buf(),
            cause: cause.to_string(),
        })
}

/* ------------------------------- archive -------------------------------- */

/// One member of an `ar` archive.
struct ArMember<'a> {
    name: String,
    data: &'a [u8],
}

/// Extract `usr/bin/ludex` from a Debian package.
pub fn extract_deb_binary(deb: &[u8], member: &str) -> Result<Vec<u8>, PortableError> {
    let members = ar_members(deb)?;
    let data = members
        .iter()
        .find(|entry| entry.name.starts_with("data.tar"))
        .ok_or_else(|| PortableError::Archive {
            cause: "the package has no data.tar member".to_string(),
        })?;
    let reader = data_reader(data)?;
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|error| PortableError::Archive {
        cause: format!("the package payload is not a tar archive ({error})"),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| PortableError::Archive {
            cause: format!("the package payload is damaged ({error})"),
        })?;
        let path = entry
            .path()
            .map_err(|error| PortableError::Archive {
                cause: format!("a package entry has no usable path ({error})"),
            })?
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();
        if path != member {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .take(MAX_BINARY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| PortableError::Archive {
                cause: format!("{member} could not be read ({error})"),
            })?;
        if bytes.len() as u64 > MAX_BINARY_BYTES {
            return Err(PortableError::Archive {
                cause: format!("{member} is larger than {MAX_BINARY_BYTES} bytes"),
            });
        }
        return Ok(bytes);
    }
    Err(PortableError::Archive {
        cause: format!("the package has no {member}"),
    })
}

/// Walk the `!<arch>` container of a `.deb`.
fn ar_members(bytes: &[u8]) -> Result<Vec<ArMember<'_>>, PortableError> {
    if !bytes.starts_with(b"!<arch>\n") {
        return Err(PortableError::Archive {
            cause: "the download is not a Debian package (no ar signature)".to_string(),
        });
    }
    let mut members = Vec::new();
    let mut offset = 8usize;
    while offset + 60 <= bytes.len() {
        let header = &bytes[offset..offset + 60];
        if &header[58..60] != b"`\n" {
            return Err(PortableError::Archive {
                cause: "the Debian package has a damaged entry header".to_string(),
            });
        }
        let name = String::from_utf8_lossy(&header[0..16])
            .trim()
            .trim_end_matches('/')
            .to_string();
        let size: usize = String::from_utf8_lossy(&header[48..58])
            .trim()
            .parse()
            .map_err(|error| PortableError::Archive {
                cause: format!("the Debian package entry {name:?} has no size ({error})"),
            })?;
        let start = offset + 60;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| PortableError::Archive {
                cause: format!("the Debian package entry {name:?} is truncated"),
            })?;
        members.push(ArMember {
            name,
            data: &bytes[start..end],
        });
        // Members are aligned on a 2-byte boundary.
        offset = end + (size % 2);
    }
    Ok(members)
}

/// Decompress a `data.tar` member.
fn data_reader<'a>(member: &'a ArMember<'a>) -> Result<Box<dyn Read + 'a>, PortableError> {
    match member.name.as_str() {
        "data.tar" => Ok(Box::new(member.data)),
        "data.tar.gz" => Ok(Box::new(flate2::read::GzDecoder::new(member.data))),
        "data.tar.zst" => {
            let decoder = zstd::stream::read::Decoder::new(member.data).map_err(|error| {
                PortableError::Archive {
                    cause: format!("the package payload could not be decompressed ({error})"),
                }
            })?;
            Ok(Box::new(decoder))
        }
        other => Err(PortableError::Archive {
            cause: format!(
                "the package uses an unsupported payload ({other}); the release needs data.tar.gz"
            ),
        }),
    }
}

/// Reject anything that is not a 64-bit ELF, and read the build commit.
fn validate_binary(bytes: &[u8]) -> Result<Option<String>, PortableError> {
    if bytes.len() < 64 {
        return Err(PortableError::Binary {
            cause: format!("the extracted file is only {} bytes", bytes.len()),
        });
    }
    if !bytes.starts_with(&ELF_MAGIC) {
        return Err(PortableError::Binary {
            cause: "the extracted file is not an ELF executable".to_string(),
        });
    }
    // EI_CLASS 2 = ELFCLASS64.
    if bytes.get(4) != Some(&2) {
        return Err(PortableError::Binary {
            cause: "the extracted file is not a 64-bit executable".to_string(),
        });
    }
    Ok(find_build_commit(bytes))
}

/// `LUDEX_BUILD_COMMIT=<sha>` written by `build.rs`, when the binary has it.
fn find_build_commit(bytes: &[u8]) -> Option<String> {
    let prefix = install_health::BUILD_STAMP_PREFIX.as_bytes();
    let start = bytes
        .windows(prefix.len())
        .position(|window| window == prefix)?
        + prefix.len();
    let rest = &bytes[start..];
    let end = rest
        .iter()
        .position(|byte| !byte.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    let commit = std::str::from_utf8(&rest[..end]).ok()?.trim();
    (!commit.is_empty() && commit.len() <= 64).then(|| commit.to_string())
}

/// Staged path for a version. The version comes from the network, so it is
/// validated before it becomes a file name.
pub fn staging_path(updates_dir: &Path, version: &str) -> Result<PathBuf, PortableError> {
    let version = version.trim().trim_start_matches('v');
    let usable = !version.is_empty()
        && version.len() <= 32
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
    if !usable {
        return Err(PortableError::Manifest {
            cause: format!("the release manifest carries an unusable version {version:?}"),
        });
    }
    Ok(updates_dir.join(format!("ludex-{version}")))
}

/// SHA-256 of the artifact, lowercase hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/* ------------------------------ logging --------------------------------- */

/// Fields of one update attempt (#312).
///
/// The summary is the line written to `logs/ludex.log`: URLs lose their query
/// and userinfo, everything is single-line, and no signature or key material
/// is ever included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptFields {
    pub kind: &'static str,
    pub version: String,
    pub current_version: String,
    pub endpoint: String,
    pub url: Option<String>,
    pub target: Option<String>,
    /// `verified`, `failed`, `missing` or `skipped`.
    pub signature: Option<&'static str>,
    pub sha256: Option<String>,
    pub failure: Option<String>,
}

impl AttemptFields {
    #[must_use]
    pub fn new(kind: &'static str, version: &str, current_version: &str, endpoint: &str) -> Self {
        Self {
            kind,
            version: version.to_string(),
            current_version: current_version.to_string(),
            endpoint: endpoint.to_string(),
            url: None,
            target: None,
            signature: None,
            sha256: None,
            failure: None,
        }
    }

    /// `key=value` pairs, safe to log.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = String::new();
        push_field(&mut out, "kind", self.kind);
        push_field(&mut out, "version", &self.version);
        push_field(&mut out, "current", &self.current_version);
        push_field(&mut out, "endpoint", &scrub_url(&self.endpoint));
        if let Some(url) = &self.url {
            push_field(&mut out, "artifact", &scrub_url(url));
        }
        if let Some(target) = &self.target {
            push_field(&mut out, "target", target);
        }
        if let Some(signature) = self.signature {
            push_field(&mut out, "signature", signature);
        }
        if let Some(sha256) = &self.sha256 {
            push_field(&mut out, "sha256", sha256);
        }
        if let Some(failure) = &self.failure {
            push_field(&mut out, "failure", &scrub_text(failure));
        }
        out
    }
}

fn push_field(out: &mut String, key: &str, value: &str) {
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(key);
    out.push('=');
    let quote = value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '"');
    if !quote {
        out.push_str(value);
        return;
    }
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push('\''),
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out.push('"');
}

/* ---------------------------- production path ---------------------------- */

/// Download, verify and install the portable update for the available version.
///
/// Returns the installed binary; the caller owns the UI status and event.
pub async fn run(
    app: &AppHandle,
    status: &UpdateStatus,
    expected: &str,
    target: &Path,
) -> Result<Installed, String> {
    let state = app
        .try_state::<AppState>()
        .ok_or_else(|| "Could not read the application state.".to_string())?;
    let endpoint = endpoint_for(status.channel);
    let mut attempt = AttemptFields::new("portable", expected, &status.current_version, endpoint);
    attempt.target = Some(target.display().to_string());
    tracing::info!(attempt = %attempt.summary(), "application update attempt");

    let result = install_portable(app, &state, &mut attempt, target).await;
    match &result {
        Ok(installed) => {
            attempt.version = installed.version.clone();
            attempt.signature = Some("verified");
            attempt.sha256 = Some(installed.sha256.clone());
            tracing::info!(attempt = %attempt.summary(), "application update installed");
        }
        Err(error) => {
            attempt.failure = Some(error.message());
            tracing::warn!(
                stage = error.stage(),
                attempt = %attempt.summary(),
                "application update failed"
            );
        }
    }
    result.map_err(|error| error.message())
}

async fn install_portable(
    app: &AppHandle,
    state: &AppState,
    attempt: &mut AttemptFields,
    target: &Path,
) -> Result<Installed, PortableError> {
    if cfg!(debug_assertions) {
        return Err(PortableError::Refused {
            reason: "this is a development build, and self-update only replaces release binaries"
                .to_string(),
        });
    }

    let endpoint = attempt.endpoint.clone();
    let body = fetch_text(&state.http, &endpoint, MAX_MANIFEST_BYTES).await?;
    let manifest = ReleaseManifest::parse(&body)?;
    let asset = manifest.asset(DEB_ASSET)?;
    attempt.url = Some(asset.url.clone());
    attempt.version = manifest.version.clone();
    if manifest.version.trim() == attempt.current_version.trim() {
        return Err(PortableError::Manifest {
            cause: format!(
                "the release manifest still points at {} which is the version already running",
                manifest.version
            ),
        });
    }

    let updates_dir = state.app_data.join(STAGING_DIR);
    let staging = staging_path(&updates_dir, &manifest.version)?;
    let pubkey = configured_pubkey(app);
    if pubkey.is_empty() {
        return Err(PortableError::Signature {
            cause: "this build has no release signing key".to_string(),
        });
    }

    let artifact = download(&state.http, &asset.url, |downloaded, total, speed_bps| {
        let percent = total
            .filter(|total| *total > 0)
            .map(|total| (downloaded as f64 / total as f64 * 100.0).clamp(0.0, 100.0));
        let _ = app.emit(
            "update://progress",
            UpdateProgress {
                downloaded,
                total,
                percent,
                speed_bps,
            },
        );
    })
    .await?;
    let sha256 = sha256_hex(&artifact);
    attempt.sha256 = Some(sha256.clone());
    tracing::info!(
        bytes = artifact.len(),
        sha256 = %sha256,
        "portable update artifact downloaded"
    );

    let plan = PortablePlan {
        version: manifest.version.clone(),
        signature: asset.signature.clone(),
        pubkey,
        target: target.to_path_buf(),
        staging,
    };
    // #186: record the expectation right before the binary is replaced, so a
    // crash during the replace is visible on the next start but a failed
    // download or a bad signature never claims an update happened.
    install_health::record_pending_update(app, &manifest.version);
    let installed = install_verified(&SystemFs, &plan, &artifact, &MinisignVerifier)
        .inspect_err(|_| install_health::clear_pending_update(app))?;
    tracing::info!(
        target = %installed.target.display(),
        bytes = installed.bytes,
        backup = installed
            .backup
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        commit = installed.commit.as_deref().unwrap_or("unknown"),
        "portable update binary replaced"
    );
    Ok(installed)
}

/// Fetch a small text document with a size cap.
async fn fetch_text(
    http: &reqwest::Client,
    url: &str,
    limit: usize,
) -> Result<String, PortableError> {
    let response = http
        .get(url)
        .timeout(MANIFEST_TIMEOUT)
        .send()
        .await
        .map_err(|cause| PortableError::Download {
            url: scrub_url(url),
            cause: cause.to_string(),
        })?;
    if !response.status().is_success() {
        return Err(PortableError::Download {
            url: scrub_url(url),
            cause: format!("the server answered {}", response.status()),
        });
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|cause| PortableError::Download {
            url: scrub_url(url),
            cause: cause.to_string(),
        })?;
    if bytes.len() > limit {
        return Err(PortableError::Download {
            url: scrub_url(url),
            cause: format!("the answer is larger than {limit} bytes"),
        });
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| PortableError::Manifest {
        cause: "the release manifest is not UTF-8".to_string(),
    })
}

/// Stream the release artifact, reporting progress like the plugin path.
///
/// `on_progress` is called at most four times per second and once more with
/// the final size, so a fast download still ends at 100 %.
async fn download(
    http: &reqwest::Client,
    url: &str,
    mut on_progress: impl FnMut(u64, Option<u64>, u64),
) -> Result<Vec<u8>, PortableError> {
    let response = http
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|cause| PortableError::Download {
            url: scrub_url(url),
            cause: cause.to_string(),
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(PortableError::Download {
            url: scrub_url(url),
            cause: format!("the server answered {status}"),
        });
    }
    let total = response.content_length();
    let mut response = response;
    let mut bytes = Vec::with_capacity(total.unwrap_or(0).min(MAX_ARTIFACT_BYTES) as usize);
    let mut last_emit = Instant::now() - PROGRESS_INTERVAL;
    let mut last_time = Instant::now();
    let mut last_bytes = 0usize;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|cause| PortableError::Download {
            url: scrub_url(url),
            cause: cause.to_string(),
        })?
    {
        if bytes.len() as u64 + chunk.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(PortableError::Download {
                url: scrub_url(url),
                cause: format!("the artifact is larger than {MAX_ARTIFACT_BYTES} bytes"),
            });
        }
        bytes.extend_from_slice(&chunk);
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            let downloaded = bytes.len() as u64;
            let elapsed = last_time.elapsed().as_secs_f64();
            let speed_bps = if elapsed > 0.0 && bytes.len() >= last_bytes {
                ((bytes.len() - last_bytes) as f64 / elapsed).round() as u64
            } else {
                0
            };
            on_progress(downloaded, total, speed_bps);
            last_emit = Instant::now();
            last_time = last_emit;
            last_bytes = bytes.len();
        }
    }
    // A final event, so a download that finished inside one throttle window
    // still reaches 100 % in the UI.
    if !bytes.is_empty() {
        on_progress(bytes.len() as u64, total, 0);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /* ------------------------------ fixtures ------------------------------ */

    fn fake_binary(marker: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 128];
        bytes[..4].copy_from_slice(&ELF_MAGIC);
        bytes[4] = 2; // ELFCLASS64
        bytes[5] = 1; // little endian
        bytes[16] = 2; // ET_EXEC, whatever: only the first bytes are checked
        bytes.extend_from_slice(marker);
        bytes
    }

    fn push_ar(out: &mut Vec<u8>, name: &str, data: &[u8]) {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            name,
            0,
            0,
            0,
            "100644",
            data.len()
        );
        assert_eq!(header.len(), 60);
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }

    /// Build the smallest real `.deb` shape Tauri publishes: ar + data.tar.*.
    fn build_deb(binary: &[u8], compression: &str) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_path("usr/bin/ludex").unwrap();
            header.set_size(binary.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, binary).unwrap();
            builder.finish().unwrap();
        }
        let payload = match compression {
            "gz" => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                encoder.write_all(&tar_bytes).unwrap();
                encoder.finish().unwrap()
            }
            "zst" => zstd::stream::encode_all(&tar_bytes[..], 3).unwrap(),
            "none" => tar_bytes,
            _ => panic!("unknown compression {compression}"),
        };
        let name = match compression {
            "none" => "data.tar".to_string(),
            other => format!("data.tar.{other}"),
        };
        let mut out = b"!<arch>\n".to_vec();
        push_ar(&mut out, "debian-binary", b"2.0\n");
        push_ar(&mut out, &name, &payload);
        out
    }

    struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "ludex-portable-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    struct FakeVerifier {
        outcome: Result<(), String>,
        calls: Cell<usize>,
    }

    impl FakeVerifier {
        fn ok() -> Self {
            Self {
                outcome: Ok(()),
                calls: Cell::new(0),
            }
        }

        fn refusing() -> Self {
            Self {
                outcome: Err("bad signature".to_string()),
                calls: Cell::new(0),
            }
        }
    }

    impl ArtifactVerifier for FakeVerifier {
        fn verify(&self, _artifact: &[u8], _signature: &str, _pubkey: &str) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            self.outcome.clone()
        }
    }

    /// Real filesystem except for `probe_writable`, which always fails.
    struct DeniedFs;

    impl UpdateFs for DeniedFs {
        fn probe_writable(&self, _dir: &Path) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Permission denied (os error 13)",
            ))
        }
        fn write_synced(&self, path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
            SystemFs.write_synced(path, bytes, mode)
        }
        fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            SystemFs.rename(from, to)
        }
        fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
            SystemFs.sync_dir(dir)
        }
    }

    /// Real filesystem, but the n-th rename fails.
    struct FailingRenameFs {
        fail_on: usize,
        calls: Cell<usize>,
    }

    impl UpdateFs for FailingRenameFs {
        fn probe_writable(&self, dir: &Path) -> std::io::Result<()> {
            SystemFs.probe_writable(dir)
        }
        fn write_synced(&self, path: &Path, bytes: &[u8], mode: u32) -> std::io::Result<()> {
            SystemFs.write_synced(path, bytes, mode)
        }
        fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            let call = self.calls.get() + 1;
            self.calls.set(call);
            if call == self.fail_on {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "rename refused by the test",
                ));
            }
            SystemFs.rename(from, to)
        }
        fn sync_dir(&self, dir: &Path) -> std::io::Result<()> {
            SystemFs.sync_dir(dir)
        }
    }

    fn plan_for(root: &TempRoot, target: PathBuf) -> PortablePlan {
        PortablePlan {
            version: "0.4.25".to_string(),
            signature: "c2lnbmF0dXJl".to_string(),
            pubkey: "cHVia2V5".to_string(),
            target,
            staging: root.path.join("updates").join("ludex-0.4.25"),
        }
    }

    /* ------------------------------ manifest ------------------------------ */

    #[test]
    fn the_manifest_resolves_the_deb_asset() {
        let manifest = ReleaseManifest::parse(
            r#"{
                "version": "0.4.25",
                "platforms": {
                    "linux-x86_64": {"signature": "cw==", "url": "https://example.test/Ludex.AppImage"},
                    "linux-x86_64-deb": {"signature": "ZGVi", "url": "https://example.test/Ludex.deb"},
                    "linux-x86_64-rpm": {"signature": "cnBt", "url": "https://example.test/Ludex.rpm"}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(manifest.version, "0.4.25");
        let asset = manifest.asset(DEB_ASSET).unwrap();
        assert_eq!(asset.url, "https://example.test/Ludex.deb");
        assert_eq!(asset.signature, "ZGVi");
        // A release without a deb entry is refused with the asset named.
        let minimal = ReleaseManifest::parse(r#"{"version":"0.4.25","platforms":{}}"#).unwrap();
        let error = minimal.asset(DEB_ASSET).unwrap_err();
        assert_eq!(error.stage(), "manifest");
        assert!(error.message().contains("linux-x86_64-deb"));
        // Malformed JSON is a manifest error, not a panic.
        assert_eq!(ReleaseManifest::parse("{").unwrap_err().stage(), "manifest");
    }

    #[test]
    fn staging_paths_are_built_from_validated_versions() {
        let dir = Path::new("/data/updates");
        assert_eq!(
            staging_path(dir, "0.4.25").unwrap(),
            PathBuf::from("/data/updates/ludex-0.4.25")
        );
        assert_eq!(
            staging_path(dir, "v0.4.25-edge.310").unwrap(),
            PathBuf::from("/data/updates/ludex-0.4.25-edge.310")
        );
        // A remote version is never allowed to walk the path.
        assert!(staging_path(dir, "../../etc/passwd").is_err());
        assert!(staging_path(dir, "").is_err());
        assert!(staging_path(dir, &"1".repeat(64)).is_err());
    }

    /* ------------------------------ archive ------------------------------- */

    #[test]
    fn a_deb_payload_is_read_with_every_compression_the_bundler_uses() {
        let binary = fake_binary(b"LUDEX_BUILD_COMMIT=abc123\n");
        for compression in ["gz", "zst", "none"] {
            let deb = build_deb(&binary, compression);
            let extracted = extract_deb_binary(&deb, DEB_BINARY)
                .unwrap_or_else(|error| panic!("{compression}: {}", error.message()));
            assert_eq!(extracted, binary, "{compression}");
        }
    }

    #[test]
    fn a_package_without_the_binary_is_refused_before_any_write() {
        let deb = build_deb(&fake_binary(b""), "gz");
        let error = extract_deb_binary(&deb, "usr/bin/other").unwrap_err();
        assert_eq!(error.stage(), "archive");
        assert!(error.message().contains("usr/bin/other"));
        assert!(error.message().contains("Nothing was replaced"));

        let error = extract_deb_binary(b"not a package", DEB_BINARY).unwrap_err();
        assert!(error.message().contains("not a Debian package"));

        let xz = {
            let mut out = b"!<arch>\n".to_vec();
            push_ar(&mut out, "debian-binary", b"2.0\n");
            push_ar(&mut out, "data.tar.xz", b"not really xz");
            out
        };
        let error = extract_deb_binary(&xz, DEB_BINARY).unwrap_err();
        assert!(error.message().contains("data.tar.xz"));
    }

    #[test]
    fn only_a_64_bit_elf_is_accepted() {
        let good = fake_binary(b"LUDEX_BUILD_COMMIT=deadbeef\n");
        assert_eq!(validate_binary(&good).unwrap().as_deref(), Some("deadbeef"));
        // The marker is optional: an older build still updates.
        assert_eq!(validate_binary(&fake_binary(b"")).unwrap(), None);

        let mut not_elf = good.clone();
        not_elf[0] = 0;
        assert!(validate_binary(&not_elf).is_err());
        let mut not_64 = good.clone();
        not_64[4] = 1;
        assert!(validate_binary(&not_64).is_err());
        assert!(validate_binary(b"\x7fELF").is_err());
        assert!(validate_binary(b"").is_err());
    }

    /* ------------------------------ install ------------------------------- */

    #[test]
    fn a_verified_update_replaces_the_binary_and_keeps_a_backup() {
        let root = TempRoot::new("replace");
        let target = root.path.join("ludex");
        fs::write(&target, b"the old build").unwrap();
        let plan = plan_for(&root, target.clone());
        let binary = fake_binary(b"LUDEX_BUILD_COMMIT=newcommit\n");
        let deb = build_deb(&binary, "gz");
        let verifier = FakeVerifier::ok();

        let installed =
            install_verified(&SystemFs, &plan, &deb, &verifier).expect("install succeeds");

        assert_eq!(verifier.calls.get(), 1);
        assert_eq!(installed.version, "0.4.25");
        assert_eq!(installed.target, target);
        assert_eq!(installed.commit.as_deref(), Some("newcommit"));
        assert_eq!(installed.bytes, binary.len());
        assert_eq!(installed.sha256, sha256_hex(&deb));
        assert_eq!(fs::read(&target).unwrap(), binary);
        let backup = installed.backup.expect("the old binary is kept");
        assert_eq!(fs::read(&backup).unwrap(), b"the old build");
        assert_eq!(backup, root.path.join("ludex.old"));
        // The verified copy is staged for a manual install.
        assert_eq!(fs::read(&plan.staging).unwrap(), binary);
    }

    #[test]
    fn a_bad_signature_changes_nothing() {
        let root = TempRoot::new("signature");
        let target = root.path.join("ludex");
        fs::write(&target, b"the old build").unwrap();
        let plan = plan_for(&root, target.clone());
        let deb = build_deb(&fake_binary(b""), "gz");
        let verifier = FakeVerifier::refusing();

        let error = install_verified(&SystemFs, &plan, &deb, &verifier).unwrap_err();

        assert_eq!(verifier.calls.get(), 1);
        assert_eq!(error.stage(), "signature");
        assert!(error.message().contains("signature"), "{}", error.message());
        assert!(error.message().contains("Nothing was replaced"));
        assert_eq!(fs::read(&target).unwrap(), b"the old build");
        assert!(!root.path.join("ludex.old").exists());
        assert!(!plan.staging.exists());
        // An unsigned manifest is refused before the verifier is consulted.
        let unsigned = PortablePlan {
            signature: "   ".to_string(),
            ..plan_for(&root, target.clone())
        };
        let verifier = FakeVerifier::ok();
        assert_eq!(
            install_verified(&SystemFs, &unsigned, &deb, &verifier)
                .unwrap_err()
                .stage(),
            "signature"
        );
        assert_eq!(verifier.calls.get(), 0);
    }

    #[test]
    fn a_read_only_folder_explains_what_to_run() {
        let root = TempRoot::new("denied");
        let target = root.path.join("ludex");
        fs::write(&target, b"the old build").unwrap();
        let plan = plan_for(&root, target.clone());
        let binary = fake_binary(b"");
        let deb = build_deb(&binary, "gz");

        let error = install_verified(&DeniedFs, &plan, &deb, &FakeVerifier::ok()).unwrap_err();

        assert_eq!(error.stage(), "writability");
        let message = error.message();
        assert!(message.contains(&target.display().to_string()), "{message}");
        assert!(
            message.contains(&root.path.display().to_string()),
            "{message}"
        );
        assert!(message.contains("not writable"), "{message}");
        assert!(message.contains("Permission denied"), "{message}");
        assert!(message.contains("mode "), "{message}");
        assert!(message.contains("sudo install -m755"), "{message}");
        assert!(
            message.contains(&plan.staging.display().to_string()),
            "{message}"
        );
        // The running binary and the folder are untouched (only the staged
        // copy of the verified binary was written).
        assert_eq!(fs::read(&target).unwrap(), b"the old build");
        assert!(!root.path.join("ludex.old").exists());
        assert_eq!(fs::read(&plan.staging).unwrap(), binary);
    }

    #[test]
    fn the_permission_message_names_owner_mode_and_command() {
        let permission = TargetPermission {
            target: PathBuf::from("/opt/ludex/ludex"),
            dir: PathBuf::from("/opt/ludex"),
            owner: "root:root".to_string(),
            mode: "0755".to_string(),
            cause: "Permission denied (os error 13)".to_string(),
            staged: Some(PathBuf::from(
                "/home/x/.local/share/cloud.cleeryy.ludex/updates/ludex-0.4.25",
            )),
        };
        let message = permission.message();
        assert!(message.starts_with("Could not install the update."));
        assert!(message.contains("/opt/ludex/ludex is owned by root:root (mode 0755)"));
        assert!(message.contains("/opt/ludex is not writable"));
        assert!(message.contains("Permission denied (os error 13)"));
        assert!(message.contains(
            "sudo install -m755 '/home/x/.local/share/cloud.cleeryy.ludex/updates/ludex-0.4.25' '/opt/ludex/ludex'"
        ));
        // Without a staged copy the message still says what to download.
        let anonymous = TargetPermission {
            staged: None,
            ..permission
        };
        let message = anonymous.message();
        assert!(message.contains("github.com/cleeryy/ludex/releases"));
        assert!(!message.contains("sudo install"));
    }

    #[test]
    fn a_failed_replacement_rolls_back() {
        let root = TempRoot::new("rollback");
        let target = root.path.join("ludex");
        fs::write(&target, b"the old build").unwrap();
        let plan = plan_for(&root, target.clone());
        let deb = build_deb(&fake_binary(b""), "gz");
        // Rename 1 moves the old binary aside; rename 2 installs the new one
        // and is the one that fails.
        let faulty = FailingRenameFs {
            fail_on: 2,
            calls: Cell::new(0),
        };

        let error = install_verified(&faulty, &plan, &deb, &FakeVerifier::ok()).unwrap_err();

        assert_eq!(error.stage(), "replace");
        let PortableError::Replace(failure) = &error else {
            panic!("expected a replace failure");
        };
        assert_eq!(failure.state, ReplaceState::RolledBack);
        assert!(error.message().contains("previous binary was restored"));
        assert_eq!(fs::read(&target).unwrap(), b"the old build");
        assert!(!root.path.join("ludex.old").exists());
    }

    #[test]
    fn a_replacement_that_cannot_even_start_keeps_the_old_binary() {
        let root = TempRoot::new("untouched");
        let target = root.path.join("ludex");
        fs::write(&target, b"the old build").unwrap();
        let plan = plan_for(&root, target.clone());
        let deb = build_deb(&fake_binary(b""), "gz");
        // Rename 1 (target -> .old) fails: nothing moved.
        let faulty = FailingRenameFs {
            fail_on: 1,
            calls: Cell::new(0),
        };

        let error = install_verified(&faulty, &plan, &deb, &FakeVerifier::ok()).unwrap_err();
        let PortableError::Replace(failure) = &error else {
            panic!("expected a replace failure");
        };
        assert_eq!(failure.state, ReplaceState::Untouched);
        assert!(error.message().contains("left untouched"));
        assert_eq!(fs::read(&target).unwrap(), b"the old build");
        assert!(!root.path.join("ludex.old").exists());
    }

    /* ------------------------------ transport ----------------------------- */

    #[tokio::test]
    async fn the_artifact_is_streamed_through_the_injected_transport() {
        // A local server, not the release channel: the download path is
        // exercised without touching GitHub.
        let payload = vec![7u8; 200_000];
        let server = crate::test_support::serve_bytes(payload.clone()).await;
        let http = reqwest::Client::new();
        let mut events: Vec<(u64, Option<u64>)> = Vec::new();

        let bytes = download(&http, &server.url, |downloaded, total, _speed| {
            events.push((downloaded, total));
        })
        .await
        .expect("the local server answers");

        assert_eq!(bytes, payload);
        // The last event carries the full size, so a fast download still ends
        // at 100 %.
        assert_eq!(
            events.last().copied(),
            Some((payload.len() as u64, Some(payload.len() as u64)))
        );
    }

    #[tokio::test]
    async fn a_transport_failure_is_a_download_error() {
        let http = reqwest::Client::new();
        // Nothing listens on port 1: a refused connection, no network involved.
        let error = download(
            &http,
            "http://127.0.0.1:1/ludex.deb?token=secret",
            |_, _, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.stage(), "download");
        let message = error.message();
        assert!(message.contains("The download from"), "{message}");
        // The query string never reaches the message.
        assert!(!message.contains("token=secret"), "{message}");
    }

    /* ------------------------------ logging ------------------------------- */

    #[test]
    fn attempt_summaries_scrub_urls_and_stay_single_line() {
        let mut attempt = AttemptFields::new(
            "portable",
            "0.4.25",
            "0.4.24",
            "https://github.com/cleeryy/ludex/releases/latest/download/latest.json",
        );
        attempt.url = Some(
            "https://user:tok3n@objects.example.test/Ludex.deb?X-Amz-Signature=secret#frag"
                .to_string(),
        );
        attempt.target = Some("/opt/ludex/ludex".to_string());
        attempt.signature = Some("verified");
        attempt.sha256 = Some("ab".repeat(32));
        attempt.failure =
            Some("GET https://example.test/x?token=abc failed\nsecond line".to_string());
        let summary = attempt.summary();
        assert!(summary.contains("kind=portable"), "{summary}");
        assert!(summary.contains("version=0.4.25"), "{summary}");
        assert!(summary.contains("target=/opt/ludex/ludex"), "{summary}");
        assert!(summary.contains("signature=verified"), "{summary}");
        assert!(
            summary.contains(&format!("sha256={}", "ab".repeat(32))),
            "{summary}"
        );
        // Secrets never reach the log.
        assert!(!summary.contains("tok3n"), "{summary}");
        assert!(!summary.contains("secret"), "{summary}");
        assert!(!summary.contains("token=abc"), "{summary}");
        assert!(!summary.contains('\n'), "{summary}");
        assert!(summary.contains("artifact=https://objects.example.test/Ludex.deb"));
        assert!(summary.contains("failure=\"GET https://example.test/x failed second line\""));
    }

    #[test]
    fn ids_are_resolved_from_the_passwd_table() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\ncleeryy:x:1000:1000::/home/c:/bin/sh\n";
        assert_eq!(name_from_table(passwd, 0).as_deref(), Some("root"));
        assert_eq!(name_from_table(passwd, 1000).as_deref(), Some("cleeryy"));
        assert_eq!(name_from_table(passwd, 4242), None);
        assert_eq!(name_from_table("", 0), None);
    }

    #[test]
    fn real_minisign_vectors_verify_and_tampering_does_not() {
        // Vector from the `minisign-verify` crate's own test suite: a public
        // key, a prehashed signature and the exact 4 bytes it signs. Tauri
        // wraps both in base64, the way `latest.json` and tauri.conf.json do.
        let pubkey = base64_text_of(
            "untrusted comment: minisign public key E7620F1842B4E81F
RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3",
        );
        let signature = base64_text_of(
            "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1556193335\tfile:test
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==",
        );
        MinisignVerifier
            .verify(b"test", &signature, &pubkey)
            .expect("the known-good vector verifies");
        assert!(MinisignVerifier
            .verify(b"Test", &signature, &pubkey)
            .is_err());
        // Nothing secret is logged when verification fails.
        let message = PortableError::Signature {
            cause: MinisignVerifier
                .verify(b"Test", &signature, &pubkey)
                .unwrap_err(),
        }
        .message();
        assert!(message.contains("Nothing was replaced"));
    }

    fn base64_text_of(text: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(text)
    }
}
