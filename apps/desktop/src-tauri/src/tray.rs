//! The tray belongs to the native process, independently of webview lifetimes.

use tauri::{
    menu::Menu,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Manager,
};

const TRAY_ID: &str = "dlman-tray";

#[cfg(windows)]
struct TrayDpi(std::sync::atomic::AtomicI32);

pub fn install(app: &App) -> Result<(), String> {
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    #[cfg(windows)]
    let icon = {
        let size = crate::window_icons::tray_icon_size();
        let icon = crate::window_icons::tray_image(size)?;
        app.manage(TrayDpi(std::sync::atomic::AtomicI32::new(size)));
        icon
    };
    #[cfg(not(windows))]
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("Missing tray artwork")?;

    // An empty menu also makes the tray visible on Linux before JS attaches its
    // translated actions. Left-click handling survives reloads and React replay.
    let menu = Menu::new(app).map_err(|error| error.to_string())?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("DLMan")
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            #[cfg(windows)]
            refresh_dpi(tray.app_handle());
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let result = window
            .show()
            .and_then(|_| window.unminimize())
            .and_then(|_| window.set_focus());
        if let Err(error) = result {
            tracing::warn!(%error, "Could not focus the existing DLMan window");
        }
    }
}

#[cfg(windows)]
pub fn refresh_dpi(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let Some(state) = app.try_state::<TrayDpi>() else {
        return;
    };
    // System-tray DPI follows Explorer's taskbar, not the app window's monitor.
    let size = crate::window_icons::tray_icon_size();
    if state.0.load(Ordering::Relaxed) == size {
        return;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let result = crate::window_icons::tray_image(size)
            .and_then(|icon| tray.set_icon(Some(icon)).map_err(|error| error.to_string()));
        match result {
            Ok(()) => state.0.store(size, Ordering::Relaxed),
            Err(error) => tracing::warn!(%error, "Could not refresh the Windows tray icon"),
        }
    }
}
