//! Update notifications (#213): OS banners for a new build.
//!
//! The web app owns the policy — account preference, skipped version, one
//! notice per version, "Later" window, open update center — and asks this
//! module to surface the *same* text as an OS notification when Ludex is in the
//! background. Everything here is a second gate: no popup while a game runs
//! (the overlay and badge already cover that), no popup in the Sunshine console
//! shell (Big Picture uses its in-shell toast), and no popup when the main
//! window is already in front (the in-app notice is on screen).
//!
//! The banner is informational. It never starts a check, a download or an
//! install; clicking it brings the window forward and opens the update center,
//! where the install decision stays. No action buttons are registered: a click
//! on the notification itself (`tap`) is the only interaction, which works on
//! Windows toast, macOS Notification Center and libnotify alike.
//!
//! The plugin reports `tap` only when a Rust handler is registered (see
//! [`register_click_handler`]); notifications that were shown without one would
//! be silently click-through, so the handler is installed during setup.

use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::{ActionPerformed, NotificationExt};

use crate::state::AppState;
use crate::tray;

/// Fixed identifier so a click can be matched back to this notification kind.
pub const NOTIFICATION_ID: i32 = 213;
/// Titles are short by nature; the cap keeps a compromised webview from
/// pushing a paragraph into the notification center.
const MAX_TITLE_CHARS: usize = 120;
/// Body cap, in characters.
const MAX_BODY_CHARS: usize = 240;

/// Account-level update notification channels (`updates.notifications`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsNotificationPreference {
    /// In-app notice and OS banner (desktop default).
    AppAndOs,
    /// In-app only.
    App,
    /// Nothing; the badge and update center stay.
    Off,
}

impl OsNotificationPreference {
    /// Parses the account value; unknown or empty values mean `app-and-os`,
    /// which matches the web default for a document written by an older build.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "app" => Self::App,
            "off" => Self::Off,
            _ => Self::AppAndOs,
        }
    }
}

/// Whether an OS notification may be shown right now.
///
/// Pure so the platform-shaped rules are unit-tested without a window:
/// - only `app-and-os` asks for an OS banner;
/// - a running game and the console shell never get one;
/// - a focused, restored window already shows the in-app notice.
#[must_use]
pub fn should_show(
    preference: OsNotificationPreference,
    focused: bool,
    minimized: bool,
    game_running: bool,
    console_active: bool,
) -> bool {
    if preference != OsNotificationPreference::AppAndOs {
        return false;
    }
    if game_running || console_active {
        return false;
    }
    !(focused && !minimized)
}

/// Strips control characters and bounds a string that came from the webview.
#[must_use]
pub fn sanitize_text(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Whether a notification for this version was already raised.
#[must_use]
pub fn version_already_notified(notified: Option<&str>, version: &str) -> bool {
    notified
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| value == version.trim())
}

/// Raise the OS notification for a new build (or a finished install).
///
/// `kind` is `available` (default) or `installed`. Only `available` is
/// deduplicated in `settings.json`: an install the user just ran is a one-shot
/// event of this session.
#[tauri::command]
pub fn notify_update_available(
    app: AppHandle,
    version: String,
    title: String,
    body: String,
    kind: Option<String>,
) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let preference = OsNotificationPreference::parse(&settings.account_update_notifications);

    let window = app.get_webview_window("main");
    let focused = window
        .as_ref()
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let minimized = window
        .as_ref()
        .and_then(|window| window.is_minimized().ok())
        .unwrap_or(false);
    let game_running = state
        .processes
        .lock()
        .map(|processes| !processes.is_empty())
        .unwrap_or(false);
    let console_active = app
        .try_state::<crate::console::ConsoleMode>()
        .is_some_and(|mode| mode.active);

    if !should_show(preference, focused, minimized, game_running, console_active) {
        tracing::debug!(
            focused,
            minimized,
            game_running,
            console_active,
            preference = ?preference,
            "update notification suppressed"
        );
        return Ok(false);
    }

    let version = sanitize_text(&version, 64);
    if version.is_empty() {
        return Ok(false);
    }
    let installed = kind.as_deref() == Some("installed");
    if !installed {
        // The web view already gates on `status.notify`, but a stale tab must
        // not resurrect a version the user skipped.
        if crate::updater::version_is_skipped(
            Some(&version),
            settings.skipped_update_version.as_deref(),
        ) {
            return Ok(false);
        }
        if version_already_notified(settings.notified_update_version.as_deref(), &version) {
            return Ok(false);
        }
    }
    let title = sanitize_text(&title, MAX_TITLE_CHARS);
    let body = sanitize_text(&body, MAX_BODY_CHARS);
    if title.is_empty() {
        return Ok(false);
    }

    app.notification()
        .builder()
        .id(NOTIFICATION_ID)
        .title(title)
        .body(body)
        .show()
        .map_err(|error| format!("Could not show the notification: {error}"))?;
    // Persist the dedupe only after the platform accepted the banner: a missing
    // notification daemon must not silence the version for good.
    if !installed {
        state.update_settings(|settings| {
            settings.notified_update_version = Some(version.clone());
        })?;
    }
    tracing::info!(version = %version, installed, "update notification shown");
    Ok(true)
}

/// Click handler shared by every notification kind: focus Ludex on `/updates`.
///
/// Only a click on the notification body (`tap`) is acted on. There are no
/// action buttons, so Windows toast, macOS Notification Center and Linux
/// libnotify behave the same: one click, update center open, no install.
pub fn handle_action(app: &AppHandle, performed: &ActionPerformed) {
    if performed.action_id() != "tap" {
        return;
    }
    let Some(notification) = performed.notification() else {
        return;
    };
    if notification.id() != NOTIFICATION_ID {
        return;
    }
    tray::show_window(app);
    let _ = app.emit("ui://navigate", "/updates");
}

/// Installs the click handler. Must run during setup, before the first
/// notification: the plugin only tracks a notification for clicks when a
/// handler exists.
pub fn register_click_handler(app: &AppHandle) -> Result<(), String> {
    let handle = app.clone();
    app.notification()
        .on_action(move |performed| handle_action(&handle, performed))
        .map_err(|error| format!("Could not register the notification handler: {error}"))
}

/// Clear the stored "already notified" version, e.g. after the user asked to
/// be reminded again.
pub fn clear_notified_version(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        let _ = state.update_settings(|settings| settings.notified_update_version = None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_parsing_keeps_the_desktop_default() {
        assert_eq!(
            OsNotificationPreference::parse("app-and-os"),
            OsNotificationPreference::AppAndOs
        );
        assert_eq!(
            OsNotificationPreference::parse(" app "),
            OsNotificationPreference::App
        );
        assert_eq!(
            OsNotificationPreference::parse("off"),
            OsNotificationPreference::Off
        );
        // An older document without the key, or a hand-edited file.
        assert_eq!(
            OsNotificationPreference::parse(""),
            OsNotificationPreference::AppAndOs
        );
        assert_eq!(
            OsNotificationPreference::parse("OS"),
            OsNotificationPreference::AppAndOs
        );
    }

    #[test]
    fn os_banner_only_when_the_app_is_in_the_background() {
        use OsNotificationPreference::{App, AppAndOs, Off};
        // Focused window: the in-app notice is enough.
        assert!(!should_show(AppAndOs, true, false, false, false));
        // Minimised or another app in front: the OS banner is the only signal.
        assert!(should_show(AppAndOs, false, false, false, false));
        assert!(should_show(AppAndOs, true, true, false, false));
        assert!(should_show(AppAndOs, false, true, false, false));
        // The preference gates the OS channel, never the in-app one.
        assert!(!should_show(App, false, false, false, false));
        assert!(!should_show(Off, false, false, false, false));
        // A running game and the console shell are silent: overlay/badge or
        // the in-shell toast already cover them.
        assert!(!should_show(AppAndOs, false, false, true, false));
        assert!(!should_show(AppAndOs, false, false, false, true));
        assert!(!should_show(AppAndOs, false, true, true, true));
    }

    #[test]
    fn text_from_the_webview_is_bounded_and_clean() {
        assert_eq!(
            sanitize_text("  Ludex 0.5.0 is available  ", 40),
            "Ludex 0.5.0 is available"
        );
        assert_eq!(sanitize_text("a\u{0}b\nc", 40), "abc");
        assert_eq!(sanitize_text(&"x".repeat(500), 12).chars().count(), 12);
        assert_eq!(sanitize_text("\u{7}", 10), "");
    }

    #[test]
    fn a_version_is_notified_once() {
        assert!(version_already_notified(Some("0.5.0"), "0.5.0"));
        assert!(version_already_notified(Some(" 0.5.0 "), "0.5.0"));
        assert!(!version_already_notified(Some("0.5.0"), "0.5.1"));
        assert!(!version_already_notified(None, "0.5.0"));
        assert!(!version_already_notified(Some("   "), "0.5.0"));
        // Edge builds are skipped one build at a time.
        assert!(!version_already_notified(
            Some("0.5.0-edge.179"),
            "0.5.0-edge.180"
        ));
    }
}
