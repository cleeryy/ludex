//! Spawning helpers for Ludex's internal helper processes (#181).
//!
//! Every probe Ludex runs in the background (`reg`, `powershell`, `cmd`,
//! Ludusavi, winetricks, the keychain CLIs, `xdotool`) must go through
//! [`command`] or [`capture`]. On Windows the helper applies
//! `CREATE_NO_WINDOW`, so pressing Check/Scan/Test never flashes a console
//! window in the user's face.
//!
//! Processes that are *meant* to be seen stay on plain
//! `std::process::Command::new`: the game and its emulator, an interactive
//! installer, Moonlight, and the `open`/`xdg-open`/`cmd start` URI handlers.
//! They keep their own window.

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

    /// Internal `reg`/`powershell`/`cmd` probes must use [`command`]/[`capture`].
    /// A new `Command::new("powershell")` would silently reintroduce a console
    /// flash on Windows, so this scan fails the build instead.
    ///
    /// User-visible spawns opt out with a `#181 user-visible` marker on the same
    /// line or the line above.
    #[test]
    fn internal_probes_go_through_the_helper() {
        const INTERNAL: [&str; 3] = ["reg", "powershell", "cmd"];

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
                let Some(needle) = INTERNAL
                    .iter()
                    .find(|program| line.contains(&format!("Command::new(\"{program}\")")))
                else {
                    continue;
                };
                let previous = index
                    .checked_sub(1)
                    .and_then(|index| lines.get(index))
                    .copied()
                    .unwrap_or_default();
                if line.contains("#181 user-visible") || previous.contains("#181 user-visible") {
                    continue;
                }
                offenders.push(format!(
                    "{}:{}: {} (use crate::process::command/capture)",
                    file.display(),
                    index + 1,
                    needle
                ));
            }
        }

        assert!(
            offenders.is_empty(),
            "internal probe bypasses the no-window helper:\n{}",
            offenders.join("\n")
        );
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
        // Building must keep the program while setting the flag.
        assert_eq!(command("reg").get_program(), OsStr::new("reg"));
    }
}
