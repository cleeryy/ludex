//! System tray icon (#15): show/hide, downloads, recent games and quit.
//!
//! The tray menu is rebuilt whenever the recent-games list or the install queue
//! changes, so it always reflects the local SQLite index. The tooltip reports
//! active downloads. Closing the window hides Ludex to the tray when the
//! `closeToTray` setting is on (default); quitting is explicit.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::db;
use crate::install::{self, InstallPhase};
use crate::state::AppState;

/// Tray identifier used to look the icon up when refreshing it.
pub const TRAY_ID: &str = "ludex-tray";

const MENU_SHOW: &str = "tray-show";
const MENU_DOWNLOADS: &str = "tray-downloads";
const MENU_RECENT_PREFIX: &str = "tray-recent:";
const MENU_CHECK_UPDATES: &str = "tray-check-updates";
const MENU_QUIT: &str = "tray-quit";

/// Recent games listed in the tray menu.
const RECENT_LIMIT: usize = 5;

/// Build and install the tray icon.
pub fn init(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Ludex")
        .menu(&menu)
        // Left click toggles the window, right click opens the menu.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    refresh_tooltip(app);
    Ok(())
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let show = MenuItem::with_id(app, MENU_SHOW, "Show / Hide Ludex", true, None::<&str>)?;
    let downloads = MenuItem::with_id(app, MENU_DOWNLOADS, "Downloads", true, None::<&str>)?;

    let recent = recent_submenu(app)?;

    let check = MenuItem::with_id(
        app,
        MENU_CHECK_UPDATES,
        "Check for updates",
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit Ludex", true, None::<&str>)?;

    Menu::with_items(
        app,
        &[&show, &downloads, &recent, &check, &separator, &quit],
    )
}

fn recent_submenu(app: &AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    let recent = recent_games(app);
    let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = Vec::new();
    if recent.is_empty() {
        items.push(Box::new(MenuItem::with_id(
            app,
            "tray-recent-empty",
            "No recent games yet",
            false,
            None::<&str>,
        )?));
    } else {
        for game in recent {
            let label = match game.version.as_deref() {
                Some(version) if !version.trim().is_empty() => {
                    format!("{} ({version})", game.title)
                }
                _ => game.title.clone(),
            };
            items.push(Box::new(MenuItem::with_id(
                app,
                format!("{MENU_RECENT_PREFIX}{}", game.game_id),
                label,
                true,
                None::<&str>,
            )?));
        }
    }
    let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        items.iter().map(|item| item.as_ref()).collect();
    Submenu::with_items(app, "Recent games", true, &refs)
}

fn recent_games(app: &AppHandle) -> Vec<db::InstalledGame> {
    let state = app.state::<AppState>();
    let Ok(connection) = state.db.lock() else {
        return Vec::new();
    };
    db::recent_installed(&connection, RECENT_LIMIT).unwrap_or_default()
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        MENU_SHOW => toggle_window(app),
        MENU_DOWNLOADS => {
            show_window(app);
            let _ = app.emit("ui://navigate", "/downloads");
        }
        MENU_QUIT => app.exit(0),
        MENU_CHECK_UPDATES => {
            show_window(app);
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = crate::updater::check_now(handle, true).await;
            });
        }
        other if other.starts_with(MENU_RECENT_PREFIX) => {
            if let Ok(game_id) = other[MENU_RECENT_PREFIX.len()..].parse::<i64>() {
                show_window(app);
                let _ = app.emit("ui://launch", game_id);
            }
        }
        _ => {}
    }
}

/// Show the main window, restoring it if it was minimised.
pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
}

/// Hide the main window to the tray.
pub fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

/// Left click: hide when the window is visible and focused, show otherwise.
pub fn toggle_window(app: &AppHandle) {
    let visible = app
        .get_webview_window("main")
        .map(|window| window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false))
        .unwrap_or(false);
    if visible {
        hide_window(app);
    } else {
        show_window(app);
    }
}

/// True when the user asked the window to close but Ludex should stay in the
/// tray. Reads the live settings so a change applies without a restart.
pub fn close_to_tray(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|state| state.settings_snapshot().close_to_tray)
        .unwrap_or(false)
}

/// Rebuild the tray menu (recent games changed, installs finished, …).
pub fn refresh_menu(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match build_menu(app) {
        Ok(menu) => {
            if let Err(error) = tray.set_menu(Some(menu)) {
                tracing::warn!(error = %error, "could not refresh the tray menu");
            }
        }
        Err(error) => tracing::warn!(error = %error, "could not build the tray menu"),
    }
}

/// Update the tooltip with the number of active downloads.
pub fn refresh_tooltip(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    let jobs = install::list(app).unwrap_or_default();
    let active = jobs
        .iter()
        .filter(|job| {
            matches!(
                job.phase,
                InstallPhase::Queued
                    | InstallPhase::WaitingForSpace
                    | InstallPhase::Downloading
                    | InstallPhase::Verifying
                    | InstallPhase::Extracting
                    | InstallPhase::Finalizing
            )
        })
        .count();
    let paused = jobs
        .iter()
        .filter(|job| job.phase == InstallPhase::Paused)
        .count();

    let tooltip = match (active, paused) {
        (0, 0) => "Ludex".to_string(),
        (0, 1) => "Ludex — 1 download paused".to_string(),
        (0, count) => format!("Ludex — {count} downloads paused"),
        (1, _) => "Ludex — 1 download running".to_string(),
        (count, _) => format!("Ludex — {count} downloads running"),
    };
    if let Err(error) = tray.set_tooltip(Some(&tooltip)) {
        tracing::debug!(error = %error, "could not update the tray tooltip");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn menu_ids_are_stable() {
        // The ids are part of the menu wiring; keep them from drifting silently.
        assert_eq!(super::MENU_SHOW, "tray-show");
        assert_eq!(super::MENU_DOWNLOADS, "tray-downloads");
        assert_eq!(super::MENU_QUIT, "tray-quit");
        assert_eq!(super::MENU_CHECK_UPDATES, "tray-check-updates");
        assert!(format!("{}42", super::MENU_RECENT_PREFIX).starts_with("tray-recent:"));
    }
}
