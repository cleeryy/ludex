//! Per-game Wine prefixes (#92): description, safe removal and winetricks
//! execution. Path derivation lives in [`crate::linux_gaming::prefix_for`];
//! this module only touches the filesystem.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::linux_gaming::{self, LaunchOptions, WinetricksCommand};

const MAX_LOG_BYTES: usize = 8 * 1024;
pub const WINETRICKS_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The prefix a game uses and how big it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefixInfo {
    pub game_id: i64,
    pub path: String,
    /// True when the path comes from the per-game options, not the default.
    pub custom: bool,
    pub exists: bool,
    pub size_bytes: u64,
}

pub fn info(
    game_id: i64,
    install_path: &Path,
    options: &LaunchOptions,
) -> Result<PrefixInfo, String> {
    let prefix = linux_gaming::prefix_for(install_path, &options.prefix_path)?;
    let exists = prefix.is_dir();
    let size_bytes = if exists {
        crate::paths::dir_size(&prefix)
    } else {
        0
    };
    Ok(PrefixInfo {
        game_id,
        path: prefix.to_string_lossy().to_string(),
        custom: !options.prefix_path.trim().is_empty(),
        exists,
        size_bytes,
    })
}

/// Refuse prefixes that would delete more than the game's own Wine data.
pub fn is_safe_to_remove(prefix: &Path, install_path: &Path) -> Result<(), String> {
    let prefix = prefix
        .canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", prefix.display()))?;
    let install = install_path
        .canonicalize()
        .map_err(|error| format!("Could not resolve {}: {error}", install_path.display()))?;

    if prefix == Path::new("/") {
        return Err("Refusing to delete the filesystem root as a Wine prefix.".to_string());
    }
    if prefix == install {
        return Err("The Wine prefix cannot be the install folder itself.".to_string());
    }
    if install.starts_with(&prefix) {
        return Err(format!(
            "Refusing to delete {} because the install folder lives inside it.",
            prefix.display()
        ));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Ok(home) = home.canonicalize() {
            if prefix == home {
                return Err("Refusing to delete your home folder as a Wine prefix.".to_string());
            }
        }
    }
    Ok(())
}

/// Delete a game prefix. Missing prefixes are a no-op.
pub fn remove(prefix: &Path, install_path: &Path) -> Result<(), String> {
    if !prefix.exists() {
        return Ok(());
    }
    is_safe_to_remove(prefix, install_path)?;

    if prefix.is_dir() {
        std::fs::remove_dir_all(prefix)
            .map_err(|error| format!("Could not delete {}: {error}", prefix.display()))
    } else {
        std::fs::remove_file(prefix)
            .map_err(|error| format!("Could not delete {}: {error}", prefix.display()))
    }
}

/// Outcome of a winetricks run, with a bounded log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinetricksReport {
    pub verbs: Vec<String>,
    pub success: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub output: String,
    pub duration_ms: u64,
}

/// Run `winetricks -q <verbs>` with a timeout, capturing stdout and stderr.
pub fn run_winetricks(
    command: &WinetricksCommand,
    timeout: Duration,
) -> Result<WinetricksReport, String> {
    let mut process = crate::process::command(&command.program);
    process
        .args(&command.args)
        .envs(&command.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let started = Instant::now();
    let mut child = process
        .spawn()
        .map_err(|error| format!("Could not start {}: {error}", command.program.display()))?;

    let buffer: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(spawn_reader(stdout, Arc::clone(&buffer)));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(spawn_reader(stderr, Arc::clone(&buffer)));
    }

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => return Err(format!("Could not wait for winetricks: {error}")),
        }
        if !timeout.is_zero() && started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            timed_out = true;
            break None;
        }
        std::thread::sleep(Duration::from_millis(150));
    };

    for reader in readers {
        let _ = reader.join();
    }

    let output = buffer.lock().map(|text| text.clone()).unwrap_or_default();
    let exit_code = status.and_then(|status| status.code());
    let success = !timed_out && exit_code == Some(0);
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(WinetricksReport {
        verbs: command
            .args
            .iter()
            .filter(|arg| *arg != "-q")
            .cloned()
            .collect(),
        success,
        timed_out,
        exit_code,
        output,
        duration_ms,
    })
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    sink: Arc<Mutex<String>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if let Ok(mut text) = sink.lock() {
                        text.push_str(&String::from_utf8_lossy(&buffer[..read]));
                        let len = text.len();
                        if len > MAX_LOG_BYTES {
                            let mut cut = len - MAX_LOG_BYTES;
                            while cut < text.len() && !text.is_char_boundary(cut) {
                                cut += 1;
                            }
                            text.drain(..cut);
                        }
                    }
                }
            }
        }
    })
}

/// Last bytes of a log file, for the installer panel.
pub fn read_tail(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let start = bytes.len().saturating_sub(MAX_LOG_BYTES);
    Some(String::from_utf8_lossy(&bytes[start..]).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-prefix-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prefix_info_derives_the_path_and_size() {
        let root = temp_dir("info");
        let install = root.join("game/default");
        std::fs::create_dir_all(&install).unwrap();
        let prefix = install.join(".prefix");
        std::fs::create_dir_all(prefix.join("drive_c")).unwrap();
        std::fs::write(prefix.join("drive_c/file"), b"12345").unwrap();

        let options = LaunchOptions {
            prefix_path: String::new(),
            ..LaunchOptions::default()
        };
        let prefix_info = info(7, &install, &options).unwrap();
        assert_eq!(prefix_info.path, prefix.to_string_lossy());
        assert!(prefix_info.exists);
        assert!(!prefix_info.custom);
        assert_eq!(prefix_info.size_bytes, 5);

        let custom = root.join("elsewhere");
        std::fs::create_dir_all(&custom).unwrap();
        let options = LaunchOptions {
            prefix_path: custom.to_string_lossy().to_string(),
            ..LaunchOptions::default()
        };
        let prefix_info = info(7, &install, &options).unwrap();
        assert!(prefix_info.custom);
        assert_eq!(prefix_info.path, custom.to_string_lossy());
        assert!(prefix_info.exists);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn removal_refuses_unsafe_prefixes_and_handles_missing_ones() {
        let root = temp_dir("remove");
        let install = root.join("game");
        std::fs::create_dir_all(&install).unwrap();
        let prefix = install.join(".prefix");
        std::fs::create_dir_all(prefix.join("drive_c")).unwrap();

        // Missing prefix: no-op.
        remove(&install.join("nope"), &install).unwrap();
        // The install folder itself.
        assert!(is_safe_to_remove(&install, &install).is_err());
        // An ancestor of the install folder.
        assert!(is_safe_to_remove(&root, &install).is_err());
        // Inside the install folder: fine.
        is_safe_to_remove(&prefix, &install).unwrap();

        remove(&prefix, &install).unwrap();
        assert!(!prefix.exists());
        assert!(install.exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn winetricks_runs_and_captures_output() {
        let root = temp_dir("wt");
        let script = root.join("winetricks");
        std::fs::write(
            &script,
            "#!/bin/sh\necho \"verbs: $*\"\necho \"prefix: $WINEPREFIX\"\n",
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let prefix = root.join("prefix");
        let command =
            linux_gaming::build_winetricks(&script, &["corefonts".into()], &prefix, None).unwrap();
        let report = run_winetricks(&command, Duration::from_secs(20)).unwrap();
        assert!(report.success);
        assert_eq!(report.exit_code, Some(0));
        assert!(report.output.contains("-q corefonts"));
        assert!(report
            .output
            .contains(&prefix.to_string_lossy().to_string()));
        assert!(!report.timed_out);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tail_reads_the_end_of_a_log() {
        let root = temp_dir("tail");
        let path = root.join("installer.log");
        std::fs::write(&path, "x".repeat(MAX_LOG_BYTES + 100)).unwrap();
        let tail = read_tail(&path).unwrap();
        assert_eq!(tail.len(), MAX_LOG_BYTES);
        assert!(read_tail(&root.join("missing.log")).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
