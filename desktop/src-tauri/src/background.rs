//! Running in the background: closing the window keeps Talaria alive, so
//! notifications still arrive (D3).
//!
//! What Close does, by platform and by what the desktop offers:
//!
//!   * macOS — the window hides; the app stays in the dock and the menu bar
//!     (tray.rs). Clicking the dock icon brings it back (`RunEvent::Reopen`).
//!   * Linux with a tray — the window hides; the tray icon has Open and Quit.
//!     A tray needs two things: the appindicator library, which is opened
//!     here by `dlopen` BEFORE any tray call (libappindicator-sys panics when
//!     it is missing), and a `org.kde.StatusNotifierWatcher` on the bus. The
//!     watcher is followed live (linux.rs), so turning on GNOME's AppIndicator
//!     extension later takes effect at once.
//!   * Linux without one (stock GNOME) — the window MINIMIZES instead. A
//!     hidden window with no icon anywhere would be a process nobody can see
//!     or quit; minimized, it stays in the dash and Alt+Tab. A one-time
//!     notice says how to quit. The dash's (or a dock's) Quit is one of the
//!     ways: the GTK application is registered as `app.talaria.desktop` and
//!     exports `app.quit` (lib.rs `install_quit_action`), which GNOME's
//!     `shell_app_request_quit` activates before it would fall back to
//!     deleting each window (the same close request as ×).
//!   * The tray host leaving while the window is closed to it (the GNOME
//!     AppIndicator extension turned off, updated or crashed) — the window
//!     comes back, with a notice, since its icon is gone (`tray_host_changed`).
//!   * Windows — Close quits, as before: it has no single-instance guard yet,
//!     so a hidden window would let a Start-menu relaunch open a second
//!     process on the same WebView2 profile (W2-D).
//!   * `closeToBackground` off (Settings → Desktop) — Close quits everywhere.
//!
//! Quit is always one step away: Ctrl+Q (Cmd+Q on macOS) in every webview,
//! the switcher's Quit, the launcher's Quit, the tray's Quit, and on Linux
//! the desktop's own (GNOME's dash and Alt+Tab).

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

use tauri::{AppHandle, Manager, Window};

use crate::ShellState;

/// What closing the window does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tray,
    MenuBar,
    Minimize,
    None,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tray => "tray",
            Self::MenuBar => "menu-bar",
            Self::Minimize => "minimize",
            Self::None => "none",
        }
    }
}

/// A tray host owns `org.kde.StatusNotifierWatcher` right now (Linux).
static TRAY_HOST: AtomicBool = AtomicBool::new(false);

/// The appindicator library opens, probed once. The same names, in the same
/// order, libappindicator-sys tries: when this says yes, its loader cannot
/// panic.
#[cfg(target_os = "linux")]
pub fn tray_library() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        for name in ["libayatana-appindicator3.so.1", "libappindicator3.so.1"] {
            // SAFETY: these are the libraries the tray crate itself loads
            // a moment later; opening one runs only its own initializers.
            if unsafe { libloading::Library::new(name) }.is_ok() {
                eprintln!("talaria-desktop: tray library {name}");
                return true;
            }
        }
        eprintln!(
            "talaria-desktop: no appindicator library (libayatana-appindicator3); no tray icon"
        );
        false
    })
}

#[cfg(not(target_os = "linux"))]
pub fn tray_library() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| cfg!(target_os = "macos"))
}

/// The bus said a tray host came or went (linux.rs). A new one gets the tray
/// icon now; a vanished one sends Close back to minimizing, and gives back a
/// window that was hidden to it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn tray_host_changed(owned: bool) {
    let was = TRAY_HOST.swap(owned, Ordering::Relaxed);
    if was != owned {
        eprintln!(
            "talaria-desktop: tray host {}",
            if owned { "present" } else { "gone" }
        );
    }
    let Some(app) = crate::notify::app() else {
        return;
    };
    let handle = app.clone();
    if owned && tray_library() {
        let _ = app.run_on_main_thread(move || {
            crate::tray::ensure(&handle);
        });
    } else if was && !owned {
        let _ = app.run_on_main_thread(move || tray_gone(&handle));
    }
}

/// The tray host left while the window may be hidden to it. With its icon
/// gone, a hidden window is a process nobody can see or reach: bring the
/// window back (without stealing focus — there is no activation token) and
/// say why. Main thread.
fn tray_gone(app: &AppHandle) {
    let Some(window) = app.get_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(true) {
        return;
    }
    eprintln!("talaria-desktop: the tray went away with the window closed to it; showing it");
    present_main(app, None);
    notice(app, TRAY_GONE_NOTICE);
}

/// In setup, once the app is registered: the menu-bar item on macOS; on
/// Linux the tray, if the bus already showed a host before the app existed
/// (a later host arrives through `tray_host_changed`).
pub fn init(app: &AppHandle) {
    if cfg!(target_os = "macos")
        || (cfg!(target_os = "linux") && TRAY_HOST.load(Ordering::Relaxed) && tray_library())
    {
        crate::tray::ensure(app);
    }
}

pub fn close_to_background(app: &AppHandle) -> bool {
    app.try_state::<ShellState>()
        .is_some_and(|s| s.settings.lock().unwrap().close_to_background)
}

/// What Close does at this moment, before the setting is applied.
pub fn mode(app: &AppHandle) -> Mode {
    if cfg!(target_os = "windows") {
        return Mode::None;
    }
    if cfg!(target_os = "macos") {
        return Mode::MenuBar;
    }
    if TRAY_HOST.load(Ordering::Relaxed) && tray_library() && crate::tray::exists(app) {
        Mode::Tray
    } else {
        Mode::Minimize
    }
}

/// The window is closing (the WM's close, or the themed titlebar's ×, which
/// calls `close()`). Keep the process when the setting and the desktop allow.
pub fn close_requested(window: &Window, api: &tauri::CloseRequestApi) {
    let app = window.app_handle();
    crate::commands::remember_all(app);
    if !close_to_background(app) {
        eprintln!("talaria-desktop: closing (close to background is off)");
        return;
    }
    match mode(app) {
        Mode::None => {}
        Mode::Tray | Mode::MenuBar => {
            api.prevent_close();
            if let Err(e) = window.hide() {
                eprintln!("talaria-desktop: hiding the window: {e}");
            }
            eprintln!(
                "talaria-desktop: closed to the background ({})",
                mode(app).as_str()
            );
            notice_once(app, mode(app));
        }
        Mode::Minimize => {
            api.prevent_close();
            if let Err(e) = crate::commands::minimize_main(app) {
                eprintln!("talaria-desktop: {e}");
            }
            eprintln!(
                "talaria-desktop: closed to the background (minimized: no tray on this desktop)"
            );
            notice_once(app, Mode::Minimize);
        }
    }
}

/// The first background close says what happened and how to quit, once
/// per install (Linux; a Mac app staying in the dock is what people expect).
fn notice_once(app: &AppHandle, mode: Mode) {
    if !cfg!(target_os = "linux") {
        return;
    }
    let state = app.state::<ShellState>();
    {
        let mut settings = state.settings.lock().unwrap();
        if settings.background_notice_shown {
            return;
        }
        settings.background_notice_shown = true;
        if let Err(e) = crate::settings::save(&state.settings_path, &settings) {
            eprintln!("talaria-desktop: {e}");
        }
    }
    notice(
        app,
        match mode {
            Mode::Tray => TRAY_NOTICE,
            _ => MINIMIZE_NOTICE,
        },
    );
}

/// What the first close to the tray says.
const TRAY_NOTICE: &str = "Closing the window keeps Talaria running in the tray so notifications arrive. Quit: the tray menu, the dock's Quit, Ctrl+Q, or the switcher's Quit. Change it in Settings → Desktop.";

/// What the first close to a minimized window says (no tray on this desktop).
const MINIMIZE_NOTICE: &str = "Talaria keeps running so notifications arrive. Quit: the dock's Quit, Ctrl+Q, or the switcher's Quit. Change it in Settings → Desktop.";

/// What the window coming back from a vanished tray says.
const TRAY_GONE_NOTICE: &str = "The tray icon went away, so Talaria's window is back. Closing it now minimizes it. Quit: the dock's Quit, Ctrl+Q, or the switcher's Quit.";

/// One of the shell's own banners, about the window itself (Linux). Each is
/// a re-alert: a later notice is shown afresh, not folded into an old one.
fn notice(app: &AppHandle, body: &'static str) {
    eprintln!("talaria-desktop: background notice: {body}");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let post = crate::notify::Post {
            label: "main".into(),
            instance: String::new(),
            title: "Talaria is still running".into(),
            body: Some(body.into()),
            tag: "background-notice".into(),
            href: None,
            kind: None,
            renotify: true,
            source: crate::notify::Source::Page,
        };
        if let Err(e) = crate::notify::post(&app, post).await {
            eprintln!("talaria-desktop: the background notice: {e}");
        }
    });
}

/// The one way the window comes forward — the tray, the dock (Reopen), a
/// second launch (Activate), a notification click. Main thread.
///
/// It never `show()`s a visible window: on Linux tao's show is gtk
/// `show_all`, which re-shows every hidden child webview, and wry's vertical
/// GtkBox then splits the window between them. A hidden window is shown with
/// GTK's own `show` (the toplevel only), and the view is put back after.
/// With an xdg-activation token, GTK hands it to the compositor, which is
/// what lets a Wayland window take focus from another app (D7).
pub fn present_main(app: &AppHandle, token: Option<String>) {
    let Some(window) = app.get_window("main") else {
        return;
    };
    let state = app.state::<ShellState>();
    let _ = window.unminimize();
    // The shell asked for the window back: as far as the suspend clock is
    // concerned it is back, focus event or not.
    state.self_minimized.store(false, Ordering::Relaxed);
    let hidden = !window.is_visible().unwrap_or(true);
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        match window.gtk_window() {
            Ok(gtk_window) => {
                if hidden {
                    gtk_window.show();
                }
                if let Some(token) = token.as_deref() {
                    gtk_window.set_startup_id(token);
                }
                gtk_window.present();
            }
            Err(e) => {
                eprintln!("talaria-desktop: the GTK window: {e}");
                let _ = window.set_focus();
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = token;
        if hidden {
            let _ = window.show();
        }
        let _ = window.set_focus();
    }
    if hidden {
        let view = *state.view.lock().unwrap();
        if let Err(e) = crate::commands::set_view(app, view) {
            eprintln!("talaria-desktop: {e}");
        }
    }
    crate::commands::focus_view(app);
    crate::commands::window_returned(app, false);
}

/// Quit from anywhere: remember every page, then exit.
pub fn quit(app: &AppHandle) {
    crate::commands::remember_all(app);
    eprintln!("talaria-desktop: quit");
    app.exit(0);
}
