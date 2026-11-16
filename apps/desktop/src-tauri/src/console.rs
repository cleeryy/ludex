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
//!
//! #227: the session is live, not startup-only. A second `Ludex.exe --console`
//! is forwarded by the single-instance plugin to the running process, which
//! promotes itself with [`promote`] instead of only showing the desktop shell.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Listener, Manager};

/// CLI flag that starts the console shell instead of the desktop window.
pub const CONSOLE_FLAG: &str = "--console";
/// CLI flag that keeps a console session windowed (useful on a desktop).
pub const WINDOWED_FLAG: &str = "--windowed";
/// Default Sunshine app name for the console entry.
pub const DEFAULT_APP_NAME: &str = "Ludex";
/// Default in-console quit chord; any key combination understood by the web UI.
pub const DEFAULT_EXIT_SHORTCUT: &str = "f12";
/// Temp file proving a `--console` launch reached this process (#227).
pub const LAUNCH_RECEIPT_FILE: &str = "ludex-console-launch.json";
/// Keep the receipt tiny: the last launches are enough to prove a swallow.
const LAUNCH_RECEIPT_LINES: usize = 16;

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
        let rest: Vec<String> = args
            .into_iter()
            .skip(1)
            .map(|argument| argument.as_ref().to_string())
            .collect();
        Self::from_forwarded_argv(&rest)
    }

    /// Parse an argv forwarded by the single-instance plugin (#227).
    ///
    /// Every element is inspected: on Windows the forwarded argv starts with
    /// the executable path, but the flag is unambiguous either way.
    pub fn from_forwarded_argv(argv: &[String]) -> Self {
        let mut active = false;
        let mut windowed = false;
        for argument in argv {
            match argument.as_str() {
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

/// Live console state (#227): a second process can promote this one.
#[derive(Debug, Default)]
pub struct ConsoleSession {
    active: AtomicBool,
    fullscreen: AtomicBool,
}

impl ConsoleSession {
    pub fn new(mode: ConsoleMode) -> Self {
        Self {
            active: AtomicBool::new(mode.active),
            fullscreen: AtomicBool::new(mode.fullscreen),
        }
    }

    pub fn mode(&self) -> ConsoleMode {
        ConsoleMode {
            active: self.active.load(Ordering::SeqCst),
            fullscreen: self.fullscreen.load(Ordering::SeqCst),
        }
    }

    /// Switch the running instance into console mode (or update `--windowed`).
    pub fn activate(&self, fullscreen: bool) {
        self.fullscreen.store(fullscreen, Ordering::SeqCst);
        self.active.store(true, Ordering::SeqCst);
    }
}

/// What the running instance must do with a forwarded argv (#227).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceAction {
    /// No `--console`: the usual "show the desktop shell" behaviour.
    ShowWindow,
    /// Promote the running desktop instance to a console session.
    Promote { fullscreen: bool },
    /// A console session is already active: only bring it forward.
    FocusConsole,
}

/// Decide what a second instance launch means. Pure so the matrix is tested
/// without spawning processes.
pub fn second_instance_action(argv: &[String], current: ConsoleMode) -> InstanceAction {
    let requested = ConsoleMode::from_forwarded_argv(argv);
    if !requested.active {
        return InstanceAction::ShowWindow;
    }
    if current.active {
        InstanceAction::FocusConsole
    } else {
        InstanceAction::Promote {
            fullscreen: requested.fullscreen,
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

/// The executable a published `--console` command should point at on this
/// machine (#227). Used to flag a stale Sunshine entry.
pub fn expected_executable() -> Option<PathBuf> {
    let current = std::env::current_exe().ok();
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    desktop_executable(current.as_deref(), appimage.as_deref())
}

/// True when the app was started as the Sunshine console app.
pub fn active(app: &AppHandle) -> bool {
    app.try_state::<ConsoleSession>()
        .map(|session| session.mode().active)
        .unwrap_or(false)
}

/// Apply the console window: no decorations on any platform and fullscreen
/// unless `--windowed` was passed. Best effort: a headless test run has no
/// window and must not crash the app. Returns true when `main` existed.
pub fn apply_window(app: &AppHandle) -> bool {
    let mode = app
        .try_state::<ConsoleSession>()
        .map(|session| session.mode())
        .unwrap_or_default();
    if !mode.active {
        return app.get_webview_window("main").is_some();
    }
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("console mode has no main window to configure yet");
        return false;
    };
    if let Err(error) = window.set_decorations(false) {
        tracing::warn!(error = %error, "could not remove the window decorations");
    }
    if mode.fullscreen {
        if let Err(error) = window.set_fullscreen(true) {
            tracing::warn!(error = %error, "could not enter fullscreen");
        }
    }
    true
}

/// Promote the running desktop instance into a console session (#227).
///
/// The Sunshine app is published as `Ludex.exe --console`; if Ludex already
/// runs in the tray, the second process never reaches `setup` and the running
/// one must become the console instead of just showing the desktop shell.
pub fn promote(app: &AppHandle, fullscreen: bool) {
    let Some(session) = app.try_state::<ConsoleSession>() else {
        tracing::warn!(
            fullscreen,
            "console promotion arrived before the session state existed; the next launch retries"
        );
        return;
    };
    let previous = session.mode();
    session.activate(fullscreen);
    let mode = session.mode();
    tracing::info!(
        fullscreen = mode.fullscreen,
        was_active = previous.active,
        "promoting the running instance to console mode"
    );

    remove_tray(app);
    if !apply_window(app) {
        tracing::warn!(
            "console promotion: the main window is not created yet; retrying after tauri://window-created"
        );
        let retry = app.clone();
        app.clone().once("tauri://window-created", move |_event| {
            if apply_window(&retry) {
                tracing::info!("console promotion: main window created, retrying");
                show_console(&retry, true);
            }
        });
    }
    show_console(app, true);
}

/// A second `--console` while a console session runs only focuses it.
pub fn focus(app: &AppHandle) {
    tracing::info!("console session already active; focusing it");
    if !apply_window(app) {
        tracing::warn!("console focus: the main window does not exist yet");
    }
    show_console(app, false);
}

/// Remove the tray icon: a console session has no desktop shell to hide.
fn remove_tray(app: &AppHandle) {
    if app.remove_tray_by_id(crate::tray::TRAY_ID).is_some() {
        tracing::info!("removed the tray icon for the console session");
    }
}

/// Show and focus `main`, and land the web shell on the Console (#267).
///
/// `HashRouter` owns the SPA route, so the hash assignment survives a webview
/// that has not registered its `ui://navigate` listener yet; the event covers
/// the already-mounted shell.
fn show_console(app: &AppHandle, navigate: bool) {
    crate::tray::show_window(app);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if navigate {
        if let Err(error) = window.eval("window.location.hash = '#/console';") {
            tracing::warn!(error = %error, "could not navigate the console shell by hash");
        }
        if let Err(error) = app.emit_to("main", "ui://navigate", "/console") {
            tracing::debug!(error = %error, "could not emit the console navigation event");
        }
    }
}

/* --------------------------- launch receipt (#227) ------------------------ */

/// One receipt line. Pure so the shape is tested without touching the disk.
pub fn launch_receipt_line(pid: u32, argv: &[String], exe: Option<&Path>, at: &str) -> String {
    let receipt = serde_json::json!({
        "pid": pid,
        "argv": argv,
        "exe": exe.map(|path| path.to_string_lossy().to_string()),
        "at": at,
    });
    receipt.to_string()
}

/// Append a `--console` launch receipt under the system temp folder.
///
/// stderr is discarded in a release Windows build, so a launch that never
/// reaches the UI would otherwise be silent. Returns the file used.
pub fn write_launch_receipt(argv: &[String]) -> Option<PathBuf> {
    let path = std::env::temp_dir().join(LAUNCH_RECEIPT_FILE);
    let executable = std::env::current_exe().ok();
    let line = launch_receipt_line(
        std::process::id(),
        argv,
        executable.as_deref(),
        &crate::util::now_rfc3339(),
    );
    if let Err(error) = append_receipt(&path, &line) {
        tracing::warn!(path = %path.display(), error = %error, "could not write the console launch receipt");
        return None;
    }
    Some(path)
}

/// Append one line, keeping only the most recent [`LAUNCH_RECEIPT_LINES`].
fn append_receipt(path: &Path, line: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.len() >= LAUNCH_RECEIPT_LINES {
        lines.drain(..lines.len() - LAUNCH_RECEIPT_LINES + 1);
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    std::fs::write(path, text)
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

    fn argv<const N: usize>(values: [&str; N]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
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
    fn a_second_instance_launch_is_decided_without_the_ui() {
        let console = ConsoleMode {
            active: true,
            fullscreen: true,
        };
        let windowed_console = ConsoleMode {
            active: true,
            fullscreen: false,
        };
        assert_eq!(
            second_instance_action(&argv(["Ludex.exe"]), ConsoleMode::default()),
            InstanceAction::ShowWindow
        );
        assert_eq!(
            second_instance_action(&argv(["Ludex.exe", "--console"]), ConsoleMode::default()),
            InstanceAction::Promote { fullscreen: true }
        );
        assert_eq!(
            second_instance_action(
                &argv(["Ludex.exe", "--console", "--windowed"]),
                ConsoleMode::default()
            ),
            InstanceAction::Promote { fullscreen: false }
        );
        // A running console session only gets focused, even with other flags.
        assert_eq!(
            second_instance_action(&argv(["Ludex.exe", "--console"]), console),
            InstanceAction::FocusConsole
        );
        assert_eq!(
            second_instance_action(
                &argv(["Ludex.exe", "--console", "--windowed"]),
                windowed_console
            ),
            InstanceAction::FocusConsole
        );
        // A plain desktop launch while the console runs still shows the shell.
        assert_eq!(
            second_instance_action(&argv(["Ludex.exe"]), console),
            InstanceAction::ShowWindow
        );
    }

    #[test]
    fn the_forwarded_argv_is_scanned_even_without_argv0() {
        assert_eq!(
            ConsoleMode::from_forwarded_argv(&argv(["--console"])),
            ConsoleMode {
                active: true,
                fullscreen: true
            }
        );
        assert_eq!(
            ConsoleMode::from_forwarded_argv(&[]),
            ConsoleMode::default()
        );
    }

    #[test]
    fn the_session_can_be_promoted_live() {
        // #227: startup-only state made a second launch a no-op.
        let session = ConsoleSession::new(ConsoleMode::default());
        assert_eq!(session.mode(), ConsoleMode::default());
        session.activate(true);
        assert!(session.mode().active);
        assert!(session.mode().fullscreen);
        // `--windowed` on an already-active session updates the mode.
        session.activate(false);
        assert!(session.mode().active);
        assert!(!session.mode().fullscreen);
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

    #[test]
    fn the_launch_receipt_is_json_and_stays_small() {
        let dir = fixture("receipt");
        let path = dir.join("ludex-console-launch.json");
        for pid in 0..LAUNCH_RECEIPT_LINES as u32 + 4 {
            let line = launch_receipt_line(
                pid,
                &argv(["ludex", "--console"]),
                Some(Path::new("/opt/ludex")),
                "2026-01-01T00:00:00Z",
            );
            append_receipt(&path, &line).unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), LAUNCH_RECEIPT_LINES);
        // The newest line is kept; every line stays a valid JSON object.
        let newest: serde_json::Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!(newest["pid"], LAUNCH_RECEIPT_LINES + 3);
        assert_eq!(newest["argv"][1], "--console");
        assert_eq!(newest["exe"], "/opt/ludex");
        for line in lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(value.get("at").is_some());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
