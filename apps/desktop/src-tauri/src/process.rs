//! Spawning helpers for Ludex's internal helper processes (#181).
//!
//! Every probe Ludex runs in the background (`reg`, `powershell`, `cmd`,
//! Ludusavi, winetricks, the keychain CLIs, `xdotool`) must go through
//! [`command`] or [`capture`]. On Windows the helper applies
//! `CREATE_NO_WINDOW`, so pressing Check/Scan/Test never flashes a console
//! window in the user's face.
//!
//! Processes that are *meant* to be seen stay on plain
//! `std::process::Command::new`: the game, an interactive installer, Moonlight,
//! and the `open`/`xdg-open`/`cmd start` URI handlers. They keep their own
//! window and carry a `#181 user-visible` marker so the source scan below does
//! not mistake them for a forgotten internal probe.
//!
//! ## Windows validation
//!
//! CI runs on Linux, so the window behaviour itself cannot be asserted here:
//! `std::process::Command` has no getter for creation flags. Two checks stand
//! in for it: the source scan proves no literal internal spawn bypasses the
//! helper, and the `#[cfg(windows)]` test proves the flag value the helper
//! applies. A real Windows build still needs a manual pass over Settings →
//! Saves scan, Settings → Streaming test, a Library scan, "Make it run" on a
//! missing redistributable (silent MSI/EXE), a silent game installer, and a ROM
//! launch. No console may flash; only the deliberately interactive game
//! installer may show its own window.

use std::ffi::OsStr;
use std::process::{Command, Stdio};

/// `CREATE_NO_WINDOW` from `winbase.h`. The child console application runs
/// without a console window.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The flags applied to an internal command on Windows.
///
/// `std::process::Command` only exposes a setter for creation flags, never a
/// getter, so this is the single value the helper feeds to `CommandExt` and the
/// unit test asserts on. Window behaviour itself cannot be observed on Linux;
/// the source scan test below covers bypasses instead.
#[cfg(windows)]
fn internal_creation_flags() -> u32 {
    CREATE_NO_WINDOW
}

/// A command for an internal helper that must not show a console window.
///
/// Standard streams are left untouched so callers can pipe stdin (for example
/// `secret-tool store`). Use [`capture`] for probes that only read output.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    hide_console(&mut command);
    command
}

/// A command for an internal probe: no console window, no stdin, piped output.
pub fn capture(program: impl AsRef<OsStr>) -> Command {
    let mut command = command(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Apply `CREATE_NO_WINDOW` to an existing command. No-op outside Windows.
pub fn hide_console(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(internal_creation_flags());
    }
    #[cfg(not(windows))]
    {
        // Nothing to do: Unix has no console-window concept. Keeping the
        // parameter used avoids an `unused_variables` warning.
        let _ = command;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    #[test]
    fn command_keeps_program_and_args() {
        let mut command = command("reg");
        command.args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"]);
        assert_eq!(command.get_program(), OsStr::new("reg"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("query"),
                OsStr::new(r"HKCU\Software\Valve\Steam"),
                OsStr::new("/v"),
                OsStr::new("SteamPath"),
            ]
        );
    }

    #[test]
    fn capture_keeps_program_and_args() {
        let mut command = capture("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"]);
        assert_eq!(command.get_program(), OsStr::new("powershell"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("-NoProfile"),
                OsStr::new("-NonInteractive"),
                OsStr::new("-Command"),
                OsStr::new("exit 0"),
            ]
        );
    }

    /// Internal helper and installer probes must use [`command`]/[`capture`]
    /// (or [`hide_console`]). A new `Command::new("powershell")` or
    /// `Command::new("msiexec")` would silently reintroduce a console flash on
    /// Windows, so this scan fails the build instead.
    ///
    /// User-visible spawns opt out with a `#181 user-visible` marker on the
    /// same line or the line above. Comment-only lines are ignored: they cannot
    /// spawn anything.
    #[test]
    fn internal_probes_go_through_the_helper() {
        // Every program Ludex may run in the background: registry and shell
        // probes, plus the installers and service/network tools an internal
        // helper could reach for.
        const INTERNAL: [&str; 12] = [
            "reg",
            "powershell",
            "cmd",
            "msiexec",
            "winget",
            "choco",
            "scoop",
            "dism",
            "sc",
            "netsh",
            "schtasks",
            "taskkill",
        ];

        let mut files = Vec::new();
        source_files(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );

        let mut offenders = Vec::new();
        for file in files {
            // The helper's own documentation mentions the pattern; skip it.
            if file.file_name().is_some_and(|name| name == "process.rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let lines: Vec<&str> = text.lines().collect();
            for (index, line) in lines.iter().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let Some(program) = literal_program(line) else {
                    continue;
                };
                if !INTERNAL.contains(&program.as_str()) {
                    continue;
                }
                let previous = index
                    .checked_sub(1)
                    .and_then(|index| lines.get(index))
                    .copied()
                    .unwrap_or_default();
                if line.contains("#181 user-visible") || previous.contains("#181 user-visible") {
                    continue;
                }
                offenders.push(format!(
                    "{}:{}: Command::new(\"{program}\") (use crate::process::command/capture)",
                    file.display(),
                    index + 1
                ));
            }
        }

        assert!(
            offenders.is_empty(),
            "internal probe bypasses the no-window helper:\n{}",
            offenders.join("\n")
        );
    }

    /// Program name of a literal `Command::new("…")` call on this line.
    ///
    /// Paths are reduced to their file name and a trailing `.exe` is dropped,
    /// so `Command::new("C:\\Windows\\System32\\reg.exe")` is caught too.
    fn literal_program(line: &str) -> Option<String> {
        const PREFIX: &str = "Command::new(\"";
        let start = line.find(PREFIX)? + PREFIX.len();
        let rest = &line[start..];
        let end = rest.find('"')?;
        let raw = rest[..end].to_ascii_lowercase();
        let name = raw.rsplit(['\\', '/']).next().unwrap_or(&raw);
        let name = name.trim_end_matches(".exe");
        (!name.is_empty()).then(|| name.to_string())
    }

    fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                source_files(&path, out);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                out.push(path);
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn internal_commands_request_create_no_window() {
        let flags = internal_creation_flags();
        assert_eq!(flags, CREATE_NO_WINDOW);
        assert_ne!(flags & CREATE_NO_WINDOW, 0);
        // Every helper path the audit migrated must keep the program and be
        // built through the flag-setting constructors.
        assert_eq!(command("reg").get_program(), OsStr::new("reg"));
        assert_eq!(
            capture("powershell").get_program(),
            OsStr::new("powershell")
        );
        assert_eq!(capture("msiexec").get_program(), OsStr::new("msiexec"));
        assert_eq!(capture("winget").get_program(), OsStr::new("winget"));
        let mut installer = Command::new("setup.exe");
        hide_console(&mut installer);
        assert_eq!(installer.get_program(), OsStr::new("setup.exe"));
    }
}
