//! Window chrome (#111): client-side decorations on Windows and Linux.
//!
//! The base `tauri.conf.json` creates the main window with `decorations: false`
//! on Windows and Linux, and the web UI draws a title bar inside its existing
//! top bar. macOS keeps the native title bar and traffic lights: the platform
//! config `tauri.macos.conf.json` re-declares the main window with
//! `decorations: true` (Tauri merge-patches platform configs with RFC 7396, so
//! arrays are replaced instead of merged — hence the full window object there),
//! and everything in this module is a no-op on macOS.
//!
//! Trade-offs that the code alone does not show (see `docs/WINDOW-CHROME.md`):
//!
//! - **Windows 11 Snap Layouts** need a native `WM_NCHITTEST` answer of
//!   `HTMAXBUTTON` over the maximise button, plus a window narrower than 500
//!   effective pixels before Windows offers the flyout. A web-drawn button can
//!   give neither: WebView2 handles the hit-test before Tauri sees it, and
//!   `minWidth: 1024` is far above the 500 px threshold. The flyout is therefore
//!   unavailable while the custom title bar is on, and "Use the native title bar"
//!   in Settings → Appearance restores it. Implementing it would need a Win32
//!   child-window overlay (for example `tauri-plugin-frame`) or a custom
//!   `HTMAXBUTTON` hit-test; both are out of scope for #111.
//! - **Toggling decorations at runtime** works without a restart: tao rewrites
//!   the window styles (`WS_CAPTION`, `WS_SIZEBOX`) and Tauri re-attaches its
//!   undecorated resize handler through `set_decorations`. The client area
//!   grows or shrinks by the caption height once, so the window keeps its outer
//!   size across the switch.
//! - **Resizing** keeps working while undecorated: `RESIZABLE` keeps
//!   `WS_THICKFRAME` in the window styles, and both tao (`WM_NCHITTEST`, all
//!   eight edges) and Tauri's runtime (`undecorated_resizing`) answer with the
//!   matching resize border. No extra `startResizeDragging` handles are needed.
//! - **Windows 11 corners and shadow**: `shadow` stays at its default `true`, so
//!   the undecorated window keeps the DWM shadow and rounded corners that come
//!   from the standard overlapped window styles. This is expected from the
//!   window flags, not a manual test: the Windows 10/11 matrix in
//!   `docs/WINDOW-CHROME.md` still has to be run by hand.

use tauri::{AppHandle, Manager};

/// Platforms where Ludex draws its own title bar.
pub const fn supports_custom_title_bar() -> bool {
    cfg!(any(target_os = "windows", target_os = "linux"))
}

/// Show the native title bar (`true`) or the custom one (`false`).
///
/// The base config already creates an undecorated window, so a `false` call on a
/// fresh window is a no-op. On macOS the native chrome is never removed, even if
/// an older settings file asks for it.
pub fn apply(app: &AppHandle, native_title_bar: bool) {
    if !supports_custom_title_bar() {
        return;
    }
    let Some(window) = app.get_webview_window("main") else {
        tracing::warn!("cannot apply the title bar preference: no main window");
        return;
    };

    let decorations = native_title_bar;
    // Skip the round trip when the window already matches: `set_decorations`
    // rebuilds the frame and briefly resizes the client area.
    if window.is_decorated().unwrap_or(!decorations) == decorations {
        return;
    }

    if let Err(error) = window.set_decorations(decorations) {
        tracing::warn!(error = %error, decorations, "could not change the window decorations");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_title_bar_follows_the_platform() {
        let expected = cfg!(any(target_os = "windows", target_os = "linux"));
        assert_eq!(supports_custom_title_bar(), expected);
    }
}
