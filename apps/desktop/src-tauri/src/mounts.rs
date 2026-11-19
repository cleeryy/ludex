//! Mount-aware install and download folders (#290).
//!
//! A library can sit on a NAS export, an external SSD or a second disk. This
//! module wraps the shared decision in [`crate::bridge::mount`] with the desktop's
//! real probe (`/proc/self/mountinfo`, `/etc/fstab`, systemd units) and adds
//! the messages the install pipeline shows:
//!
//! - [`host_status`] powers Settings → Locations and the install picker, so an
//!   unplugged drive reads "offline" instead of "folder will be created".
//! - [`ensure`] tries the matching `.automount` once when the folder is asked
//!   for explicitly (Check button, install start).
//! - [`offline_failure`] turns a mid-download `ENOENT`/`EIO` on an absent mount
//!   into a pause with an explicit message instead of a corrupt staging folder.
//!
//! Tests inject a fixture probe through [`crate::bridge::mount::ensure`]; nothing
//! here touches a real mount or systemctl during tests.

use std::path::Path;

use crate::bridge::mount::{
    ensure as ensure_with_probe, AutomountKind, AutomountPlan, EnsuredMount, MountIdentity,
    MountStatus, SystemProbe,
};

/// Real probe for Linux hosts.
#[derive(Debug, Default, Clone, Copy)]
pub struct HostProbe;

impl SystemProbe for HostProbe {
    fn mountinfo(&self) -> Result<String, String> {
        std::fs::read_to_string("/proc/self/mountinfo")
            .map_err(|error| format!("could not read /proc/self/mountinfo: {error}"))
    }

    fn fstab(&self) -> String {
        std::fs::read_to_string("/etc/fstab").unwrap_or_default()
    }

    fn automount_units(&self) -> Vec<String> {
        let mut units = Vec::new();
        for directory in [
            "/run/systemd/generator",
            "/run/systemd/system",
            "/etc/systemd/system",
        ] {
            let Ok(entries) = std::fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".automount") && !units.contains(&name) {
                    units.push(name);
                }
            }
        }
        units
    }

    fn run_plan(&self, plan: &AutomountPlan) -> Result<(), String> {
        let (program, argument) = match plan.kind {
            AutomountKind::SystemdUnit => (
                "systemctl",
                plan.unit
                    .clone()
                    .unwrap_or_else(|| plan.mount_point.clone()),
            ),
            AutomountKind::FstabMount => ("mount", plan.mount_point.clone()),
        };
        let output = std::process::Command::new(program)
            .arg(&argument)
            .output()
            .map_err(|error| format!("could not run {program} {argument}: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{program} {argument} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn path_exists(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn identity(&self, path: &Path) -> Option<MountIdentity> {
        let existing = crate::paths::nearest_existing(path)?;
        let metadata = std::fs::metadata(&existing).ok()?;
        let marker = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX));
        Some(MountIdentity {
            #[cfg(unix)]
            device_id: {
                use std::os::unix::fs::MetadataExt;
                metadata.dev()
            },
            #[cfg(not(unix))]
            device_id: 0,
            marker,
        })
    }
}

/// Mount state of one folder on this host, without side effects.
#[must_use]
pub fn status(path: &Path) -> MountStatus {
    crate::bridge::mount::status(&HostProbe, path)
}

/// Mount state of one folder on this host.
#[must_use]
pub fn host_status(path: &Path) -> EnsuredMount {
    ensure_with_probe(&HostProbe, path)
}

/// Checks a folder and, when its mount is absent, tries the automount once.
#[must_use]
pub fn ensure(path: &Path) -> EnsuredMount {
    host_status(path)
}

/// Message when the folder is offline, `None` when it is usable.
#[must_use]
pub fn offline_reason(path: &Path) -> Option<String> {
    let status = host_status(path).status;
    status.message().map(str::to_string)
}

/// True for the OS errors a removed drive or unmounted share produces.
///
/// `ENOENT` (2), `EIO` (5), `ENXIO` (6), `ENODEV` (19), `ESTALE` (116) and the
/// Windows drive-gone codes (`ERROR_NOT_READY` 21, `ERROR_DEVICE_NOT_CONNECTED`
/// 1167) all mean "the volume went away", not "the file is corrupt".
#[must_use]
pub fn is_mount_gone_error(error: &std::io::Error) -> bool {
    if matches!(
        error.raw_os_error(),
        Some(2) | Some(5) | Some(6) | Some(19) | Some(21) | Some(116) | Some(1167)
    ) {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("stale file handle")
        || message.contains("transport endpoint is not connected")
        || message.contains("host is down")
        || message.contains("device not ready")
}

/// Turns a failed write on an absent mount into a pausing message (#290).
///
/// `None` means the error was not a mount problem and the caller keeps its
/// existing handling. The partial archive is never discarded: the scheduler
/// pauses the job, so replugging the drive lets the user resume.
#[must_use]
pub fn offline_failure(part: &Path, error: &std::io::Error) -> Option<String> {
    offline_failure_with(&HostProbe, part, error)
}

/// [`offline_failure`] with an injected probe, so tests never read the host
/// mount table.
#[must_use]
pub fn offline_failure_with(
    probe: &dyn SystemProbe,
    part: &Path,
    error: &std::io::Error,
) -> Option<String> {
    if !is_mount_gone_error(error) {
        return None;
    }
    let directory = part.parent().unwrap_or(part);
    let ensured = ensure_with_probe(probe, directory);
    if let MountStatus::Offline {
        message,
        absent_mount: true,
        ..
    } = &ensured.status
    {
        return Some(format!(
            "The library is offline. {message} The partial archive was kept at {}; reconnect the drive or share and resume the install.",
            part.display()
        ));
    }
    // A bare ENOENT on a healthy mount is a real missing file: leave it alone.
    None
}

/// Message shown before a job starts on an offline library.
#[must_use]
pub fn offline_pause_message(directory: &Path, message: &str) -> String {
    format!(
        "The library is offline. {message} Nothing was written to {}; reconnect the drive or share and resume the install.",
        directory.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture probe: a NAS fstab entry whose mount is not up.
    struct AbsentNas {
        existing: &'static str,
    }

    impl SystemProbe for AbsentNas {
        fn mountinfo(&self) -> Result<String, String> {
            Ok("23 1 259:1 / / rw shared:1 - ext4 /dev/nvme0n1p2 rw".to_string())
        }

        fn fstab(&self) -> String {
            "nas.local:/games /mnt/nas nfs4 _netdev,noauto 0 0\n".to_string()
        }

        fn automount_units(&self) -> Vec<String> {
            Vec::new()
        }

        fn run_plan(&self, _plan: &AutomountPlan) -> Result<(), String> {
            // The mount never comes up in this fixture.
            Err("mount: network is unreachable".to_string())
        }

        fn path_exists(&self, path: &Path) -> bool {
            path == Path::new(self.existing)
        }

        fn identity(&self, _path: &Path) -> Option<MountIdentity> {
            None
        }
    }

    #[test]
    fn a_gone_mount_pauses_the_job_with_the_library_message() {
        let probe = AbsentNas {
            existing: "/mnt/nas",
        };
        let part = Path::new("/mnt/nas/games/.ludex-tmp/game.zip.part");
        let error = std::io::Error::from_raw_os_error(5);
        let message = offline_failure_with(&probe, part, &error).expect("an offline message");
        assert!(message.contains("The library is offline"));
        assert!(message.contains("/mnt/nas"));
        assert!(message.contains("partial archive was kept"));
        assert!(message.contains("resume"));
        // A non-mount error on the same volume keeps the normal handling.
        let denied = std::io::Error::from_raw_os_error(13);
        assert!(offline_failure_with(&probe, part, &denied).is_none());
    }

    #[test]
    fn mount_gone_errors_are_recognised_by_code_and_message() {
        assert!(is_mount_gone_error(&std::io::Error::from_raw_os_error(2)));
        assert!(is_mount_gone_error(&std::io::Error::from_raw_os_error(116)));
        assert!(is_mount_gone_error(&std::io::Error::other(
            "Transport endpoint is not connected"
        )));
        assert!(!is_mount_gone_error(&std::io::Error::from_raw_os_error(28)));
        assert!(!is_mount_gone_error(&std::io::Error::from_raw_os_error(13)));
    }

    #[test]
    fn a_real_missing_file_on_a_healthy_volume_is_not_an_offline_failure() {
        // /tmp is mounted (or the root filesystem): an ENOENT there is a plain
        // missing file, not an absent library mount.
        let error = std::io::Error::from_raw_os_error(2);
        assert!(offline_failure(Path::new("/tmp/ludex-mounts-test/part.zip"), &error).is_none());
    }

    #[test]
    fn a_non_mount_error_keeps_the_caller_handling() {
        let error = std::io::Error::from_raw_os_error(13);
        assert!(offline_failure(Path::new("/missing/part.zip"), &error).is_none());
    }

    #[test]
    fn the_pause_message_names_the_folder_and_the_kept_archive() {
        let message = offline_pause_message(Path::new("/mnt/nas/games"), "The mount is absent.");
        assert!(message.contains("offline"));
        assert!(message.contains("/mnt/nas/games"));
        assert!(message.contains("Nothing was written"));
    }
}
