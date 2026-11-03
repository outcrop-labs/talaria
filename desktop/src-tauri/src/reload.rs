//! Reloading an instance page.
//!
//! An instance webview has no browser chrome, so before this there was no way
//! to reload one. Reload is reached four ways:
//! - **The keys** (`RELOAD_KEYS`, an initialization script in every instance
//!   webview, beside `ESCAPE_HATCH`): Ctrl+R and F5 on Windows and Linux,
//!   Cmd+R on macOS. Adding Shift (or Ctrl+F5) skips the HTTP cache. The script
//!   is the shell's own, so it runs on any page that loads in the webview: an
//!   instance whose Talaria UI predates the reload command, a gateway's 502
//!   page, another site. Where the page may not call the shell (an origin
//!   with no grant), it reloads by itself.
//! - **The in-UI switcher's Reload page** (ui DesktopSwitcher.svelte →
//!   `desktop_reload`).
//! - **macOS: View → Reload Page** in the menu bar (`install_menu`). It is the
//!   app's own menu, not the page's, so it works on a page whose scripts never
//!   ran. A page that already handled Cmd+R stops the menu from handling it
//!   too.
//! - The engine's own keys where it has them: WebView2 reloads on F5 and Ctrl+R
//!   on a page where the script could not run (its error page).
//!
//! A reload into a page that does not come up (a restart behind Cloudflare,
//! a page that is not Talaria) comes back to the launcher with its card, on
//! every OS (loading.rs).

use tauri::{AppHandle, Manager, Webview};

use crate::ShellState;

/// Injected into every instance webview (commands.rs::create_instance_webview).
/// `scripts/desktop-reload-keys.mjs` runs it against every key it takes and
/// several it must leave alone, in `bun run check`.
pub const RELOAD_KEYS: &str = r#"
  if (window.__TAURI_INTERNALS__) {
    const mac = /Mac|iPhone|iPad/.test(navigator.userAgent);
    window.addEventListener('keydown', (e) => {
      if (e.repeat || e.altKey) return;
      const k = String(e.key || '').toLowerCase();
      // A layout whose R key types another letter (Cyrillic, Greek) still
      // reloads on the R key, as browsers do; Dvorak's R, where KeyR types p,
      // does not.
      const r = k === 'r' || (!/^[a-z]$/.test(k) && e.code === 'KeyR');
      let hard;
      if (r && (mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey)) hard = e.shiftKey;
      else if (!mac && k === 'f5' && !e.metaKey) hard = e.ctrlKey || e.shiftKey;
      else return;
      e.preventDefault();
      // A popup (macOS hands the connect popup the opener's scripts and IPC)
      // reloads itself, not the instance page behind it.
      if (window.opener) return location.reload();
      window.__TAURI_INTERNALS__.invoke('desktop_reload', { hard }).catch(() => location.reload());
    }, true);
  }
"#;

/// Reload the page that asks: an instance page reloads itself. The launcher
/// reloads the active instance's page, if there is one. `hard` skips the HTTP
/// cache where the engine can.
///
/// Async, so the reload is sent to the main thread from a tokio worker and
/// queued there. A sync command would call into WebView2 inside the caller's
/// own IPC callback (the threading note in commands.rs).
#[tauri::command]
pub async fn desktop_reload(
    app: AppHandle,
    webview: Webview,
    hard: Option<bool>,
) -> Result<(), String> {
    let target = if webview.label().starts_with("instance-") {
        webview
    } else {
        active_webview(&app).ok_or_else(|| "no instance page is open".to_string())?
    };
    reload(&target, hard.unwrap_or(false))
}

/// The page on screen, when it is an instance's.
fn active_webview(app: &AppHandle) -> Option<Webview> {
    let id = app.state::<ShellState>().active.lock().unwrap().clone()?;
    app.get_window("main")?
        .get_webview(&crate::commands::instance_label(&id))
}

fn reload(webview: &Webview, hard: bool) -> Result<(), String> {
    eprintln!(
        "talaria-desktop: reloading {}{}",
        webview.label(),
        if hard { ", skipping the cache" } else { "" }
    );
    if hard && reload_from_origin(webview) {
        return Ok(());
    }
    webview
        .reload()
        .map_err(|e| format!("reloading the page: {e}"))
}

/// The engine's own reload that skips the cache. False: none here, or the
/// webview is gone.
#[cfg(target_os = "linux")]
fn reload_from_origin(webview: &Webview) -> bool {
    webview
        .with_webview(|platform| {
            use webkit2gtk::WebViewExt;
            platform.inner().reload_bypass_cache();
        })
        .is_ok()
}

#[cfg(target_os = "macos")]
fn reload_from_origin(webview: &Webview) -> bool {
    webview
        .with_webview(|platform| {
            let view = platform.inner().cast::<objc2::runtime::AnyObject>();
            if view.is_null() {
                return;
            }
            // SAFETY: `inner` is the live WKWebView on the main thread, where
            // `with_webview` runs this. `-[WKWebView reloadFromOrigin]` takes
            // nothing and returns an autoreleased WKNavigation (or nil), which
            // is not kept.
            unsafe {
                let _: *mut objc2::runtime::AnyObject = objc2::msg_send![&*view, reloadFromOrigin];
            }
        })
        .is_ok()
}

/// WebView2 has no cache-skipping reload outside the DevTools protocol: an
/// ordinary reload.
#[cfg(target_os = "windows")]
fn reload_from_origin(_webview: &Webview) -> bool {
    false
}

/// macOS: View → Reload Page (Cmd+R) and Reload Ignoring Cache (Shift+Cmd+R)
/// at the top of the default menu's View menu.
#[cfg(target_os = "macos")]
pub fn install_menu(app: &AppHandle) {
    use tauri::menu::MenuItemKind;
    let Some(menu) = app.menu() else {
        return;
    };
    let view = menu.items().ok().and_then(|items| {
        items.into_iter().find_map(|item| match item {
            MenuItemKind::Submenu(s) if s.text().is_ok_and(|t| t == "View") => Some(s),
            _ => None,
        })
    });
    let Some(view) = view else {
        eprintln!("talaria-desktop: no View menu to add Reload to");
        return;
    };
    if let Err(e) = add_menu_items(app, &view) {
        eprintln!("talaria-desktop: adding Reload to the View menu: {e}");
        return;
    }
    app.on_menu_event(|app, event| {
        let hard = match event.id().as_ref() {
            MENU_RELOAD => false,
            MENU_RELOAD_HARD => true,
            _ => return,
        };
        match active_webview(app) {
            Some(webview) => {
                if let Err(e) = reload(&webview, hard) {
                    eprintln!("talaria-desktop: {e}");
                }
            }
            None => eprintln!("talaria-desktop: Reload Page: no instance page is open"),
        }
    });
}

#[cfg(target_os = "macos")]
fn add_menu_items(app: &AppHandle, view: &tauri::menu::Submenu<tauri::Wry>) -> tauri::Result<()> {
    use tauri::menu::{MenuItem, PredefinedMenuItem};
    let reload = MenuItem::with_id(app, MENU_RELOAD, "Reload Page", true, Some("CmdOrCtrl+R"))?;
    let hard = MenuItem::with_id(
        app,
        MENU_RELOAD_HARD,
        "Reload Ignoring Cache",
        true,
        Some("CmdOrCtrl+Shift+R"),
    )?;
    view.prepend_items(&[&reload, &hard, &PredefinedMenuItem::separator(app)?])
}

#[cfg(target_os = "macos")]
const MENU_RELOAD: &str = "reload";
#[cfg(target_os = "macos")]
const MENU_RELOAD_HARD: &str = "reload-hard";
