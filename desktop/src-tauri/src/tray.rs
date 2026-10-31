//! The tray icon (Linux, through appindicator) and the menu-bar item (macOS):
//! Open Talaria and Quit, and the unread sum as the tooltip. A left click
//! opens the window where the platform reports one (macOS; appindicator
//! only ever shows the menu).
//!
//! On Linux it is built only once background.rs has found the appindicator
//! library AND a tray host on the bus — never before, because the tray crate
//! panics without the library, and without a host an icon would be shown to
//! nobody while Close hid the window from everybody.

use std::sync::Mutex;

use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

/// The live icon, if one was built (Tauri-managed).
#[derive(Default)]
pub struct Tray(Mutex<Option<TrayIcon>>);

pub fn exists(app: &AppHandle) -> bool {
    app.try_state::<Tray>()
        .is_some_and(|t| t.0.lock().unwrap().is_some())
}

/// Build the icon if it is not up yet. Main thread. False when it could not
/// be built (Close then minimizes instead of hiding).
pub fn ensure(app: &AppHandle) -> bool {
    let Some(tray) = app.try_state::<Tray>() else {
        return false;
    };
    if tray.0.lock().unwrap().is_some() {
        return true;
    }
    match build(app) {
        Ok(icon) => {
            *tray.0.lock().unwrap() = Some(icon);
            set_unread(app, crate::notify::badge_total(app));
            eprintln!("talaria-desktop: tray icon up");
            true
        }
        Err(e) => {
            eprintln!("talaria-desktop: the tray icon: {e}");
            false
        }
    }
}

fn build(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let open = MenuItem::with_id(app, "open", "Open Talaria", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Talaria", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut builder = TrayIconBuilder::with_id("talaria")
        .tooltip("Talaria")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => crate::background::present_main(app, None),
            "quit" => crate::background::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::background::present_main(tray.app_handle(), None);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)
}

/// The tooltip carries the unread sum across every instance.
pub fn set_unread(app: &AppHandle, total: u32) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let tooltip = match total {
        0 => "Talaria".to_string(),
        1 => "Talaria — 1 unread".to_string(),
        n => format!("Talaria — {n} unread"),
    };
    // Cloned out first: the call goes through the main thread, which must
    // never wait on this lock.
    let icon = tray.0.lock().unwrap().clone();
    if let Some(icon) = icon {
        let _ = icon.set_tooltip(Some(tooltip));
    }
}
