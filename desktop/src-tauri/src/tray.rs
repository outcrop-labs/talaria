//! The tray icon (Linux, through appindicator) and the menu-bar item (macOS):
//! a row per instance with its own unread count ("Acme — 3"; a click opens
//! that instance), Open Talaria and Quit, and the unread sum — the tooltip on
//! macOS, the label beside the icon on Linux (appindicator has no tooltip:
//! tray-icon's `set_tooltip` is a no-op there; its title is the Ayatana
//! label, which the GNOME AppIndicator extension draws). The counts are each
//! instance's share of the badge (For you), so a read anywhere drops them.
//!
//! macOS draws a template image (the wing mark in black, which the menu bar
//! tints for light, dark and highlighted), and a left click shows the menu,
//! as every menu-bar item does. On Linux appindicator only ever shows the
//! menu.
//!
//! On Linux it is built only once background.rs has found the appindicator
//! library AND a tray host on the bus — never before, because the tray crate
//! panics without the library, and without a host an icon would be shown to
//! nobody while Close hid the window from everybody.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};

use tauri::{
    AppHandle, Manager,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

/// The most instance rows the menu lists.
pub const MAX_ROWS: usize = 9;

/// The live icon, if one was built (Tauri-managed), and the instance rows
/// its menu shows now.
#[derive(Default)]
pub struct Tray {
    icon: Mutex<Option<TrayIcon>>,
    rows: Mutex<Vec<(String, String)>>,
}

pub fn exists(app: &AppHandle) -> bool {
    app.try_state::<Tray>()
        .is_some_and(|t| t.icon.lock().unwrap().is_some())
}

/// Build the icon if it is not up yet. Main thread. False when it could not
/// be built (Close then minimizes instead of hiding).
pub fn ensure(app: &AppHandle) -> bool {
    let Some(tray) = app.try_state::<Tray>() else {
        return false;
    };
    if tray.icon.lock().unwrap().is_some() {
        return true;
    }
    let rows = current_rows(app);
    match build(app, &rows) {
        Ok(icon) => {
            *tray.icon.lock().unwrap() = Some(icon);
            *tray.rows.lock().unwrap() = rows;
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

/// Each registered instance's row: its menu id and its text, in registry
/// order, at most `MAX_ROWS`.
fn current_rows(app: &AppHandle) -> Vec<(String, String)> {
    let Some(state) = app.try_state::<crate::ShellState>() else {
        return Vec::new();
    };
    // One lock at a time (ShellState's discipline).
    let instances: Vec<(String, String)> = state
        .instances
        .lock()
        .unwrap()
        .iter()
        .map(|i| (i.id.clone(), i.label.clone()))
        .collect();
    let muted = state.muted.lock().unwrap().clone();
    rows(&instances, &crate::notify::badge_shares(app), &muted)
}

/// The rows, pure: `Acme — 3` with a count, `Acme` without one, and
/// `(muted)` after an instance muted in the launcher (its count still shows).
pub fn rows(
    instances: &[(String, String)],
    shares: &BTreeMap<String, u32>,
    muted: &BTreeSet<String>,
) -> Vec<(String, String)> {
    instances
        .iter()
        .take(MAX_ROWS)
        .map(|(id, label)| {
            let count = shares.get(&format!("instance-{id}")).copied().unwrap_or(0);
            let mut text = if count > 0 {
                format!("{label} — {count}")
            } else {
                label.clone()
            };
            if muted.contains(id) {
                text.push_str(" (muted)");
            }
            (format!("instance:{id}"), text)
        })
        .collect()
}

fn menu(app: &AppHandle, rows: &[(String, String)]) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::new(app)?;
    for (id, text) in rows {
        menu.append(&MenuItem::with_id(app, id, text, true, None::<&str>)?)?;
    }
    if !rows.is_empty() {
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        "open",
        "Open Talaria",
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        "quit",
        "Quit Talaria",
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

fn build(app: &AppHandle, rows: &[(String, String)]) -> tauri::Result<TrayIcon> {
    let macos = cfg!(target_os = "macos");
    let mut builder = TrayIconBuilder::with_id("talaria")
        .tooltip("Talaria")
        .menu(&menu(app, rows)?)
        .show_menu_on_left_click(macos)
        .on_menu_event(|app, event| {
            let id = event.id.as_ref();
            match id {
                "open" => crate::background::present_main(app, None),
                "quit" => crate::background::quit(app),
                _ => {
                    if let Some(instance) = id.strip_prefix("instance:") {
                        crate::commands::open_from_tray(app, instance);
                    }
                }
            }
        })
        .on_tray_icon_event(move |tray, event| {
            // macOS shows the menu on a left click; elsewhere a left click,
            // where the platform reports one, opens the window.
            if macos {
                return;
            }
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::background::present_main(tray.app_handle(), None);
            }
        });
    if macos {
        builder = builder
            .icon(tauri::include_image!("icons/tray-template.png"))
            .icon_as_template(true);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)
}

/// The instance rows may have changed (a count, an instance added, removed,
/// renamed or muted): rebuild the menu when its text differs. Never call it
/// holding a lock of the shell state.
pub fn refresh_menu(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    let icon = tray.icon.lock().unwrap().clone();
    let Some(icon) = icon else {
        return;
    };
    let rows = current_rows(app);
    {
        let mut shown = tray.rows.lock().unwrap();
        if *shown == rows {
            return;
        }
        *shown = rows.clone();
    }
    let handle = app.clone();
    // Menus are GTK (and AppKit) objects: built on the main thread.
    let _ = app.run_on_main_thread(move || match menu(&handle, &rows) {
        Ok(menu) => match icon.set_menu(Some(menu)) {
            Ok(()) => eprintln!(
                "talaria-desktop: tray menu: {}",
                rows.iter()
                    .map(|(_, text)| text.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Err(e) => eprintln!("talaria-desktop: the tray menu: {e}"),
        },
        Err(e) => eprintln!("talaria-desktop: the tray menu: {e}"),
    });
}

/// The tooltip (macOS) or the label (Linux) carries the unread sum across
/// every instance.
pub fn set_unread(app: &AppHandle, total: u32) {
    let Some(tray) = app.try_state::<Tray>() else {
        return;
    };
    // Cloned out first: the call goes through the main thread, which must
    // never wait on this lock.
    let icon = tray.icon.lock().unwrap().clone();
    let Some(icon) = icon else {
        return;
    };
    if cfg!(target_os = "linux") {
        // A label takes panel space, so it is only the count, and only when
        // there is one.
        let _ = icon.set_title(label(total));
    } else {
        let _ = icon.set_tooltip(Some(tooltip(total)));
    }
}

/// The menu-bar tooltip.
pub fn tooltip(total: u32) -> String {
    match total {
        0 => "Talaria".to_string(),
        1 => "Talaria — 1 unread".to_string(),
        n => format!("Talaria — {n} unread"),
    }
}

/// The panel label beside the Linux tray icon: the count, or nothing.
pub fn label(total: u32) -> Option<String> {
    (total > 0).then(|| total.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unread_sum_is_a_tooltip_or_a_bare_count() {
        assert_eq!(tooltip(0), "Talaria");
        assert_eq!(tooltip(1), "Talaria — 1 unread");
        assert_eq!(tooltip(7), "Talaria — 7 unread");
        assert_eq!(label(0), None);
        assert_eq!(label(12).as_deref(), Some("12"));
    }

    #[test]
    fn each_instance_is_a_row_with_its_own_count() {
        let instances = vec![
            ("a".to_string(), "Rig A".to_string()),
            ("b".to_string(), "Rig B".to_string()),
            ("c".to_string(), "Rig C".to_string()),
        ];
        let shares: BTreeMap<String, u32> =
            [("instance-a".to_string(), 2), ("instance-b".to_string(), 1)]
                .into_iter()
                .collect();
        let none = BTreeSet::new();
        assert_eq!(
            rows(&instances, &shares, &none),
            vec![
                ("instance:a".to_string(), "Rig A — 2".to_string()),
                ("instance:b".to_string(), "Rig B — 1".to_string()),
                ("instance:c".to_string(), "Rig C".to_string()),
            ]
        );
        // A read drops the count; a muted instance says so and keeps its own.
        let read: BTreeMap<String, u32> = [("instance-a".to_string(), 2)].into_iter().collect();
        let muted: BTreeSet<String> = ["a".to_string()].into_iter().collect();
        assert_eq!(
            rows(&instances, &read, &muted)
                .into_iter()
                .map(|(_, t)| t)
                .collect::<Vec<_>>(),
            vec!["Rig A — 2 (muted)", "Rig B", "Rig C"]
        );
        let many: Vec<(String, String)> =
            (0..20).map(|i| (i.to_string(), format!("I{i}"))).collect();
        assert_eq!(rows(&many, &shares, &none).len(), MAX_ROWS);
    }
}
