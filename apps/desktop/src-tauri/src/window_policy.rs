//! One window policy (#269): decorations, fullscreen, tray, close behaviour and
//! the overlay host, resolved once from `{argv, console session, settings}`.
//!
//! Before this module the same decision was spread over three places: Rust
//! applied the console window from argv (`console.rs`), routed `CloseRequested`
//! from the tray setting plus the console session (`overlay.rs`), and the web
//! guessed fullscreen from `console_mode()` plus `isFullscreen()` (#233). A
//! window state with three owners drifts; the web now reads [`WindowPolicy`]
//! from the `window_state` command and the `window://state` event instead.
//!
//! `--console` promotion (`console.rs`), Sunshine publishing and the overlay
//! are unchanged: they call [`apply`], which is the only place that touches
//! decorations and fullscreen.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::chrome;
use crate::console::ConsoleSession;
use crate::state::AppState;

/// The platform the policy is resolved for. A parameter instead of `cfg!` so
/// the truth table runs on every host: the Windows and macOS rows are checked
/// by the Linux CI job too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    Linux,
    Other,
}

/// The platform of the running build.
pub const fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::Other
    }
}

/// What a `CloseRequested` event does for a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CloseAction {
    /// The main window with close-to-tray on: hide it instead of quitting.
    HideToTray,
    /// Let the window close (a console session, or a window with no tray).
    Close,
    /// The overlay: hide it, never hide the main window (#228).
    CloseOverlay,
}

/// Where an overlay panel belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayHost {
    /// The desktop shell draws it inside the SPA.
    InApp,
    /// A console session prefers the floating window: the in-app panel is
    /// desktop-styled and sits on top of the couch surface.
    Window,
    /// The overlay is disabled in settings.
    Disabled,
}

/// The single source of truth for the main window (#269).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowPolicy {
    /// OS title bar and borders. macOS keeps its native chrome.
    pub decorations: bool,
    /// A console session started without `--windowed` is fullscreen.
    pub fullscreen: bool,
    /// A tray icon exists. A console session has none (#172).
    pub tray: bool,
    pub close_action: CloseAction,
    pub overlay_host: OverlayHost,
}

/// Everything the policy is resolved from. `argv` and the live console session
/// are collapsed into the two console fields by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyInput {
    pub console_active: bool,
    /// `true` unless the session was started or promoted with `--windowed`.
    pub console_fullscreen: bool,
    pub native_title_bar: bool,
    pub close_to_tray: bool,
    pub overlay_enabled: bool,
}

/// Resolve the policy. Pure so the truth table needs no Tauri app.
pub fn resolve(input: PolicyInput, platform: Platform) -> WindowPolicy {
    if input.console_active {
        // A console session owns its window: no chrome, no tray, and closing it
        // ends the Sunshine session for real so Moonlight sees it (#172).
        WindowPolicy {
            decorations: false,
            fullscreen: input.console_fullscreen,
            tray: false,
            close_action: CloseAction::Close,
            overlay_host: if input.overlay_enabled {
                OverlayHost::Window
            } else {
                OverlayHost::Disabled
            },
        }
    } else {
        WindowPolicy {
            // Windows/Linux draw their own title bar unless the setting asks for
            // the native one (#111); macOS always keeps the native frame.
            decorations: platform == Platform::MacOs || input.native_title_bar,
            fullscreen: false,
            tray: true,
            close_action: if input.close_to_tray {
                CloseAction::HideToTray
            } else {
                CloseAction::Close
            },
            overlay_host: if input.overlay_enabled {
                OverlayHost::InApp
            } else {
                OverlayHost::Disabled
            },
        }
    }
}

/// Resolve the policy for the running app.
pub fn current(app: &AppHandle) -> WindowPolicy {
    let mode = app
        .try_state::<ConsoleSession>()
        .map(|session| session.mode())
        .unwrap_or_default();
    let (native_title_bar, close_to_tray, overlay_enabled) = app
        .try_state::<AppState>()
        .map(|state| {
            let settings = state.settings_snapshot();
            (
                settings.native_title_bar,
                settings.close_to_tray,
                settings.overlay_enabled,
            )
        })
        .unwrap_or((false, false, false));
    resolve(
        PolicyInput {
            console_active: mode.active,
            console_fullscreen: mode.fullscreen,
            native_title_bar,
            close_to_tray,
            overlay_enabled,
        },
        current_platform(),
    )
}

/// Apply the policy to the main window. This is the only place that changes
/// decorations or fullscreen. Best effort: a headless test run has no window
/// and must not crash the app. Returns true when `main` existed.
pub fn apply(app: &AppHandle) -> bool {
    let policy = current(app);
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("the window policy has no main window to configure yet");
        return false;
    };

    if policy.decorations {
        // Never removes macOS chrome, and skips the round trip when the window
        // already matches.
        chrome::apply(app, true);
    } else if window.is_decorated().unwrap_or(true) {
        if let Err(error) = window.set_decorations(false) {
            tracing::warn!(error = %error, "could not remove the window decorations");
        }
    }

    if window.is_fullscreen().unwrap_or(false) != policy.fullscreen {
        if let Err(error) = window.set_fullscreen(policy.fullscreen) {
            tracing::warn!(error = %error, fullscreen = policy.fullscreen, "could not apply fullscreen");
        }
    }

    emit_policy(app, &policy);
    true
}

/// Emit `window://state` for the running app.
pub fn emit(app: &AppHandle) {
    let policy = current(app);
    emit_policy(app, &policy);
}

fn emit_policy(app: &AppHandle, policy: &WindowPolicy) {
    if let Err(error) = app.emit_to("main", "window://state", policy) {
        tracing::debug!(error = %error, "could not emit the window policy");
    }
}

/// Route a window close from the resolved policy. Pure so "Alt+F4 on the
/// overlay does not hide Ludex" (#228) and "a console closes for real" (#172)
/// are one test.
pub fn close_request_action(label: &str, policy: WindowPolicy) -> CloseAction {
    if label == crate::overlay::OVERLAY_LABEL {
        return CloseAction::CloseOverlay;
    }
    if label != "main" {
        return CloseAction::Close;
    }
    policy.close_action
}

/// The current policy for the web shell. The web reads this instead of
/// inferring fullscreen or close behaviour (#269).
#[tauri::command]
pub fn window_state(app: AppHandle) -> WindowPolicy {
    current(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLATFORMS: [Platform; 4] = [
        Platform::Windows,
        Platform::MacOs,
        Platform::Linux,
        Platform::Other,
    ];

    fn desktop() -> PolicyInput {
        PolicyInput {
            console_active: false,
            console_fullscreen: false,
            native_title_bar: false,
            close_to_tray: true,
            overlay_enabled: true,
        }
    }

    fn console(fullscreen: bool) -> PolicyInput {
        PolicyInput {
            console_active: true,
            console_fullscreen: fullscreen,
            ..desktop()
        }
    }

    #[test]
    fn the_truth_table_covers_console_windowed_close_and_overlay() {
        for platform in PLATFORMS {
            // A console session owns a chrome-less, tray-less window and always
            // closes for real, whatever the desktop settings say.
            for fullscreen in [true, false] {
                for close_to_tray in [true, false] {
                    for native_title_bar in [true, false] {
                        for overlay_enabled in [true, false] {
                            let input = PolicyInput {
                                close_to_tray,
                                native_title_bar,
                                overlay_enabled,
                                ..console(fullscreen)
                            };
                            let policy = resolve(input, platform);
                            assert!(!policy.decorations, "{platform:?} {input:?}");
                            assert_eq!(policy.fullscreen, fullscreen, "{platform:?} {input:?}");
                            assert!(!policy.tray, "{platform:?} {input:?}");
                            assert_eq!(
                                policy.close_action,
                                CloseAction::Close,
                                "{platform:?} {input:?}"
                            );
                            assert_eq!(
                                policy.overlay_host,
                                if overlay_enabled {
                                    OverlayHost::Window
                                } else {
                                    OverlayHost::Disabled
                                },
                                "{platform:?} {input:?}"
                            );
                        }
                    }
                }
            }

            // The desktop shell keeps the tray, the configured close action and
            // the native window frame on macOS.
            for close_to_tray in [true, false] {
                for native_title_bar in [true, false] {
                    for overlay_enabled in [true, false] {
                        let input = PolicyInput {
                            close_to_tray,
                            native_title_bar,
                            overlay_enabled,
                            ..desktop()
                        };
                        let policy = resolve(input, platform);
                        assert_eq!(
                            policy.decorations,
                            platform == Platform::MacOs || native_title_bar,
                            "{platform:?} {input:?}"
                        );
                        assert!(!policy.fullscreen, "{platform:?} {input:?}");
                        assert!(policy.tray, "{platform:?} {input:?}");
                        assert_eq!(
                            policy.close_action,
                            if close_to_tray {
                                CloseAction::HideToTray
                            } else {
                                CloseAction::Close
                            },
                            "{platform:?} {input:?}"
                        );
                        assert_eq!(
                            policy.overlay_host,
                            if overlay_enabled {
                                OverlayHost::InApp
                            } else {
                                OverlayHost::Disabled
                            },
                            "{platform:?} {input:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_console_session_never_hides_to_a_tray_it_does_not_have() {
        let policy = resolve(console(true), Platform::Windows);
        assert_eq!(policy.close_action, CloseAction::Close);
        assert!(!policy.tray);
    }

    #[test]
    fn closing_the_overlay_never_hides_the_desktop_shell() {
        // #228: the old handler called `hide_window` for any window, so Alt+F4
        // on the overlay hid Ludex.
        let desktop_policy = resolve(desktop(), Platform::Linux);
        assert_eq!(
            close_request_action(crate::overlay::OVERLAY_LABEL, desktop_policy),
            CloseAction::CloseOverlay
        );
        assert_eq!(
            close_request_action("main", desktop_policy),
            CloseAction::HideToTray
        );
        // A console session closes for real so Moonlight sees it end.
        assert_eq!(
            close_request_action("main", resolve(console(true), Platform::Linux)),
            CloseAction::Close
        );
        let windowed_close = resolve(
            PolicyInput {
                close_to_tray: false,
                ..desktop()
            },
            Platform::Linux,
        );
        assert_eq!(
            close_request_action("main", windowed_close),
            CloseAction::Close
        );
        assert_eq!(
            close_request_action("other", desktop_policy),
            CloseAction::Close
        );
    }

    #[test]
    fn the_command_shape_is_camel_case_for_the_web() {
        let policy = resolve(
            PolicyInput {
                close_to_tray: false,
                native_title_bar: false,
                overlay_enabled: true,
                ..console(false)
            },
            Platform::Windows,
        );
        let value = serde_json::to_value(policy).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "decorations": false,
                "fullscreen": false,
                "tray": false,
                "closeAction": "close",
                "overlayHost": "window",
            })
        );
        let desktop_policy = resolve(
            PolicyInput {
                close_to_tray: true,
                native_title_bar: true,
                overlay_enabled: false,
                ..desktop()
            },
            Platform::Windows,
        );
        let value = serde_json::to_value(desktop_policy).unwrap();
        assert_eq!(value["closeAction"], "hideToTray");
        assert_eq!(value["overlayHost"], "disabled");
        assert_eq!(value["decorations"], true);
    }
}
