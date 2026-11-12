//! Ludex console mode (#172): the desktop app is its own Sunshine app.
//!
//! Sunshine starts `ludex --console`, which opens Big Picture fullscreen with
//! no window chrome. Quitting that window closes the app, so Moonlight sees the
//! session end and the host returns to the desktop instead of leaving a hidden
//! tray app behind. The same binary also runs the normal desktop shell; the
//! flag is the only difference.
//!
//! Nothing here touches Sunshine directly: `streaming.rs` builds the managed
//! `Ludex` entry with [`command_line`], and the console app reuses the existing
//! Big Picture screen, controller navigation and layouts.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

/// CLI flag that starts the console shell instead of the desktop window.
pub const CONSOLE_FLAG: &str = "--console";
/// CLI flag that keeps a console session windowed (useful on a desktop).
pub const WINDOWED_FLAG: &str = "--windowed";
/// Default Sunshine app name for the console entry.
pub const DEFAULT_APP_NAME: &str = "Ludex";
/// Default in-console quit chord; any key combination understood by the web UI.
pub const DEFAULT_EXIT_SHORTCUT: &str = "f12";

/// Console runtime state, decided once from the command line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConsoleMode {
    pub active: bool,
    /// A console launched without `--windowed` goes fullscreen.
    pub fullscreen: bool,
}

impl ConsoleMode {
    /// Parse the process arguments. `argv[0]` is the executable and is skipped.
    pub fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut active = false;
        let mut windowed = false;
        for argument in args.into_iter().skip(1) {
            match argument.as_ref() {
                CONSOLE_FLAG => active = true,
                WINDOWED_FLAG => windowed = true,
                _ => {}
            }
        }
        Self {
            active,
            fullscreen: active && !windowed,
        }
    }
}

/// What a Sunshine sync publishes for the current settings (#172).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Publishing {
    /// Publish the single console app.
    pub console: bool,
    /// Publish one entry per installed game.
    pub games: bool,
}

/// Console mode publishes the single app; the expert `per_game_apps` option
/// adds the per-game entries back. With console mode off, per-game publishing
/// is always on and the console app is removed by the next sync.
pub fn publishing(console_mode: bool, per_game_apps: bool) -> Publishing {
    Publishing {
        console: console_mode,
        games: !console_mode || per_game_apps,
    }
}

/// The Sunshine app name: the configured name, or `Ludex` when empty.
pub fn app_name(configured: &str) -> String {
    let name = configured.trim();
    if name.is_empty() {
        DEFAULT_APP_NAME.to_string()
    } else {
        name.to_string()
    }
}

/// The in-console quit chord, normalised for display and comparison.
pub fn exit_shortcut(configured: &str) -> String {
    let shortcut = configured.trim().to_lowercase();
    if shortcut.is_empty() {
        DEFAULT_EXIT_SHORTCUT.to_string()
    } else {
        shortcut
    }
}

fn quote_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw.contains([' ', '\t', '"']) {
        format!("\"{}\"", raw.replace('"', "\\\""))
    } else {
        raw.into_owned()
    }
}

/// The Sunshine `cmd` for the console app: this binary plus `--console`.
///
/// A windowed console session gets `--windowed`, so the behaviour does not
/// depend on a settings file the streaming host may not share.
pub fn command_line(executable: &Path, fullscreen: bool) -> String {
    let mut command = format!("{} {CONSOLE_FLAG}", quote_path(executable));
    if !fullscreen {
        command.push(' ');
        command.push_str(WINDOWED_FLAG);
    }
    command
}

/// Sunshine's `working-dir` for the console app: the executable's folder, so a
/// portable build keeps its relative resources.
pub fn working_directory(executable: &Path) -> String {
    executable
        .parent()
        .map(|parent| parent.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Resolve the executable Sunshine must start.
///
/// An AppImage runs from a temporary mount, so `current_exe` points inside it;
/// `APPIMAGE` is the real file the user installed. Both candidates must exist
/// so a broken environment is reported instead of published.
pub fn desktop_executable(current: Option<&Path>, appimage: Option<&Path>) -> Option<PathBuf> {
    appimage
        .filter(|path| path.is_file())
        .or_else(|| current.filter(|path| path.is_file()))
        .map(Path::to_path_buf)
}

/// True when the app was started as the Sunshine console app.
pub fn active(app: &AppHandle) -> bool {
    app.try_state::<ConsoleMode>()
        .map(|mode| mode.active)
        .unwrap_or(false)
}

/// Apply the console window: no decorations on any platform and fullscreen
/// unless `--windowed` was passed. Best effort: a headless test run has no
/// window and must not crash the app.
pub fn apply_window(app: &AppHandle) {
    let mode = app
        .try_state::<ConsoleMode>()
        .map(|mode| *mode)
        .unwrap_or_default();
    if !mode.active {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("console mode has no main window to configure");
        return;
    };
    if let Err(error) = window.set_decorations(false) {
        tracing::warn!(error = %error, "could not remove the window decorations");
    }
    if mode.fullscreen {
        if let Err(error) = window.set_fullscreen(true) {
            tracing::warn!(error = %error, "could not enter fullscreen");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ludex-console-{label}-{}",
            crate::util::short_id(&uuid::Uuid::new_v4().to_string())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn args_enable_console_and_default_to_fullscreen() {
        assert_eq!(
            ConsoleMode::from_args(["ludex", "--console"]),
            ConsoleMode {
                active: true,
                fullscreen: true
            }
        );
        assert_eq!(
            ConsoleMode::from_args(["ludex", "--console", "--windowed"]),
            ConsoleMode {
                active: true,
                fullscreen: false
            }
        );
        assert_eq!(
            ConsoleMode::from_args(["ludex", "--windowed"]),
            ConsoleMode::default()
        );
        assert_eq!(ConsoleMode::from_args(["ludex"]), ConsoleMode::default());
    }

    #[test]
    fn app_name_and_exit_shortcut_fall_back_to_defaults() {
        assert_eq!(app_name("  "), DEFAULT_APP_NAME);
        assert_eq!(app_name(" Saloon "), "Saloon");
        assert_eq!(exit_shortcut(""), DEFAULT_EXIT_SHORTCUT);
        assert_eq!(exit_shortcut("  Ctrl+Shift+Q "), "ctrl+shift+q");
    }

    #[test]
    fn command_line_quotes_paths_and_keeps_the_windowed_flag() {
        assert_eq!(
            command_line(Path::new("/usr/bin/ludex"), true),
            "/usr/bin/ludex --console"
        );
        assert_eq!(
            command_line(Path::new("/opt/Ludex App/ludex"), false),
            "\"/opt/Ludex App/ludex\" --console --windowed"
        );
        assert_eq!(
            working_directory(Path::new("/opt/Ludex App/ludex")),
            "/opt/Ludex App"
        );
    }

    #[test]
    fn publishing_follows_console_mode_and_the_expert_option() {
        assert_eq!(
            publishing(true, false),
            Publishing {
                console: true,
                games: false
            }
        );
        assert_eq!(
            publishing(true, true),
            Publishing {
                console: true,
                games: true
            }
        );
        assert_eq!(
            publishing(false, true),
            Publishing {
                console: false,
                games: true
            }
        );
        assert_eq!(
            publishing(false, false),
            Publishing {
                console: false,
                games: true
            }
        );
    }

    #[test]
    fn appimage_prefers_the_real_file_and_missing_paths_are_ignored() {
        let dir = fixture("executable");
        let exe = dir.join("ludex");
        std::fs::write(&exe, b"").unwrap();
        let appimage = dir.join("Ludex.AppImage");
        std::fs::write(&appimage, b"").unwrap();

        assert_eq!(
            desktop_executable(
                Some(Path::new("/tmp/.mount_ludex/usr/bin/ludex")),
                Some(&appimage)
            ),
            Some(appimage.clone())
        );
        assert_eq!(
            desktop_executable(Some(&exe), Some(Path::new("/missing/Ludex.AppImage"))),
            Some(exe.clone())
        );
        assert_eq!(
            desktop_executable(Some(Path::new("/missing/ludex")), None),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
