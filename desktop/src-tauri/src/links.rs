//! Where a link that leaves an instance page goes.
//!
//! An instance webview is a browser tab without a browser around it. A
//! `target="_blank"` link or a `window.open` asks the engine for a new window,
//! and with no handler installed wry drops that request, so on every OS the
//! click did nothing. A plain `mailto:` link had nowhere to go on WebKitGTK,
//! which reports only that it cannot show the URL.
//!
//! Where each kind of link goes:
//! - **Another site** (http or https on any other origin) opens in the default
//!   browser. The person is signed in to that site there: Google Drive,
//!   GitHub, a source the agent read, a coding-harness sign-in.
//! - **The instance's own origin** opens as the engine's own popup window,
//!   which keeps the instance's session and `window.opener`. The MCP connect
//!   popup (`/api/mcp/oauth/start`) needs both: it signs in there, then posts
//!   back to the page that opened it and closes. The default browser has no
//!   instance session, so the connect would start signed out there.
//! - **`mailto:`, `tel:` and app links** (`zoommtg:`, `slack:`, `msteams:`…) go
//!   to the OS, which picks the handler. This applies whether the link opens a
//!   new window or navigates in place.
//! - **Refused** (logged, never opened):
//!   - `file:`, because a page must not reach the local disk;
//!   - `javascript:`, `data:`, `blob:` and `about:` as a new window, because
//!     they mean nothing outside the page;
//!   - the schemes browsers refuse to hand the OS: Chrome's denied list, the
//!     Windows handlers that web pages have abused, and remote file shares,
//!     which a link would mount.
//!
//! Neither handler opens anything itself. A popup is the engine's own
//! (`NewWindowResponse::Allow`). A hand-off runs on a blocking worker, because
//! opening waits for the launcher (`xdg-open`, `/usr/bin/open`), and the
//! handlers are called on the main thread (scripts/desktop-threading.mjs).

use tauri::{Runtime, Url, webview::NewWindowResponse};

/// What happens to a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// It stays in the app: the navigation goes ahead, or the new window is
    /// the engine's own popup.
    InApp,
    /// The default browser.
    Browser,
    /// The OS handler for its scheme.
    Os,
    /// Nowhere.
    Refused,
}

/// Schemes the engine loads itself, never the OS: pages and their frames,
/// sockets, Tauri's own protocols, the engines' internal pages.
const ENGINE: &[&str] = &[
    "http",
    "https",
    "about",
    "blob",
    "data",
    "javascript",
    "ws",
    "wss",
    "tauri",
    "ipc",
    "asset",
    "chrome",
    "chrome-error",
    "chrome-extension",
    "edge",
    "devtools",
    "applewebdata",
];

/// Schemes no instance page may send anywhere.
const REFUSED: &[&str] = &[
    // A local file. `allowed_navigation` refuses it before it gets here.
    "file",
    "vbscript",
    "view-source",
    // Chrome's denied external schemes (external_protocol_handler.cc).
    "afp",
    "disk",
    "disks",
    "hcp",
    "ie.http",
    "ms-help",
    "nntp",
    "res",
    "shell",
    "vnd.ms.radio",
    // Windows handlers abused from web pages: Follina (ms-msdt), search-ms and
    // search (a remote share dressed as a search), ms-officecmd, and the App
    // Installer that Microsoft turned off for the web.
    "ms-msdt",
    "search-ms",
    "search",
    "ms-officecmd",
    "ms-appinstaller",
    // Remote file shares that a link would mount.
    "smb",
    "cifs",
    "nfs",
    "ftp",
    "sftp",
    "dav",
    "davs",
];

/// A navigation inside an instance page: it loads, its URL goes to the OS, or
/// it is refused.
pub(crate) fn navigation(url: &Url) -> Route {
    let scheme = url.scheme();
    if REFUSED.contains(&scheme) {
        Route::Refused
    } else if ENGINE.contains(&scheme) {
        Route::InApp
    } else {
        Route::Os
    }
}

/// A new window that an instance page asked for (`target="_blank"`,
/// `window.open`). `instance` is the instance's registered URL: its origin
/// is the only one that stays in the app.
pub(crate) fn new_window(instance: &Url, url: &Url) -> Route {
    match url.scheme() {
        "http" | "https" if url.origin() == instance.origin() => Route::InApp,
        "http" | "https" => Route::Browser,
        _ => match navigation(url) {
            Route::Os => Route::Os,
            _ => Route::Refused,
        },
    }
}

/// What a log line says about a URL: its scheme and host, never its path or
/// query (a mailto: address, an OAuth code).
fn shown(url: &Url) -> String {
    match url.host_str() {
        Some(host) => format!("{}://{host}", url.scheme()),
        None => format!("{}:", url.scheme()),
    }
}

/// The new-window handler of an instance webview.
pub(crate) fn on_new_window<R: Runtime>(
    label: &str,
    instance: &Url,
    url: Url,
) -> NewWindowResponse<R> {
    match new_window(instance, &url) {
        Route::InApp => {
            eprintln!(
                "talaria-desktop: {label}: a popup on its own origin opens in the app ({})",
                url.path()
            );
            NewWindowResponse::Allow
        }
        route @ (Route::Browser | Route::Os) => {
            hand_off(label, url, route);
            NewWindowResponse::Deny
        }
        Route::Refused => {
            eprintln!(
                "talaria-desktop: {label}: refused a new window to {}",
                shown(&url)
            );
            NewWindowResponse::Deny
        }
    }
}

/// The navigation half, for `allowed_navigation`: false cancels the
/// navigation, and a URL for the OS is handed over first.
pub(crate) fn allow_navigation(label: &str, url: &Url) -> bool {
    match navigation(url) {
        Route::InApp | Route::Browser => true,
        Route::Os => {
            hand_off(label, url.clone(), Route::Os);
            false
        }
        Route::Refused => {
            eprintln!(
                "talaria-desktop: {label}: refused a navigation to {}",
                shown(url)
            );
            false
        }
    }
}

/// Give `url` to the default browser or the OS handler for its scheme, on a
/// blocking worker: `open` waits for the launcher to start or exit, so the
/// handler that asked never waits.
fn hand_off(label: &str, url: Url, route: Route) {
    let label = label.to_string();
    let to = if route == Route::Browser {
        "the default browser"
    } else {
        "the OS"
    };
    eprintln!("talaria-desktop: {label}: {} goes to {to}", shown(&url));
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
            eprintln!(
                "talaria-desktop: {label}: could not hand {} to {to}: {e}",
                shown(&url)
            );
        }
    });
}

/// Linux: an in-app popup's window closes when its page closes itself. wry
/// builds the popup (`NewWindowResponse::Allow`) as a GtkApplicationWindow
/// holding a box that holds the page's WebView. It answers the page's
/// `window.close()` by destroying only the WebView, so the window stayed,
/// empty and titled with the popup's first URL, until Talaria quit. The MCP
/// connect popup closes itself that way when it is done. The main window is
/// never touched.
#[cfg(target_os = "linux")]
pub(crate) fn close_emptied_popups(app: &tauri::AppHandle) {
    use gtk::prelude::*;
    use tauri::Manager;
    let Some(main) = app.get_window("main").and_then(|w| w.gtk_window().ok()) else {
        return;
    };
    let Some(application) = main.application() else {
        return;
    };
    application.connect_window_added(move |_, window| {
        if window == main.upcast_ref::<gtk::Window>() {
            return;
        }
        let popup = window.clone();
        window.connect_add(move |_, child| {
            let Some(content) = child.downcast_ref::<gtk::Box>() else {
                return;
            };
            let popup = popup.clone();
            content.connect_remove(move |content, _| {
                if content.children().is_empty() {
                    popup.close();
                }
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        s.parse().unwrap()
    }

    const INSTANCE: &str = "https://talaria.example.com";

    fn opened(s: &str) -> Route {
        new_window(&url(INSTANCE), &url(s))
    }

    #[test]
    fn another_site_opens_in_the_default_browser() {
        for s in [
            "https://drive.google.com/file/d/abc/view",
            "https://github.com/settings/apps/new",
            "http://example.org/",
            // Same host, other scheme or port: another origin.
            "http://talaria.example.com/",
            "https://talaria.example.com:8443/",
            "https://sub.talaria.example.com/",
        ] {
            assert_eq!(opened(s), Route::Browser, "{s}");
        }
    }

    #[test]
    fn the_instances_own_origin_stays_in_the_app() {
        for s in [
            "https://talaria.example.com/api/mcp/oauth/start?server=x&scope=me",
            "https://talaria.example.com/a/public-slug",
            "https://talaria.example.com:443/settings",
            "https://TALARIA.example.com/",
        ] {
            assert_eq!(opened(s), Route::InApp, "{s}");
        }
        // A dev instance's port is part of its origin.
        let dev = url("http://127.0.0.1:5302/");
        assert_eq!(
            new_window(&dev, &url("http://127.0.0.1:5302/mcp")),
            Route::InApp
        );
        assert_eq!(
            new_window(&dev, &url("http://127.0.0.1:5303/")),
            Route::Browser
        );
    }

    #[test]
    fn mailto_tel_and_app_links_go_to_the_os() {
        for s in [
            "mailto:someone@example.com",
            "tel:+15551234567",
            "sms:+15551234567",
            "zoommtg://zoom.us/join?confno=1",
            "slack://open",
            "msteams:/l/meetup-join/x",
            "webcal://example.com/cal.ics",
        ] {
            assert_eq!(opened(s), Route::Os, "{s}");
            assert_eq!(navigation(&url(s)), Route::Os, "{s}");
        }
    }

    #[test]
    fn a_local_file_or_a_script_is_never_opened() {
        for s in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<p>x</p>",
            "blob:https://talaria.example.com/1234",
            "about:blank",
            "vbscript:msgbox(1)",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x&crumb=location:\\\\evil\\share",
            "ms-officecmd:{}",
            "ms-appinstaller:?source=https://evil/x.appx",
            "shell:startup",
            "smb://evil.example/share",
            "ftp://example.com/file",
            "view-source:https://example.com",
        ] {
            assert_eq!(opened(s), Route::Refused, "{s}");
        }
    }

    #[test]
    fn a_navigation_loads_what_the_engine_loads_and_refuses_the_rest() {
        for s in [
            "https://accounts.google.com/o/oauth2/auth",
            "http://127.0.0.1:5302/",
            "about:blank",
            "about:srcdoc",
            "blob:https://talaria.example.com/1234",
            "data:text/html,<p>x</p>",
            "javascript:void(0)",
        ] {
            assert_eq!(navigation(&url(s)), Route::InApp, "{s}");
        }
        for s in ["file:///home/x/a.pdf", "ms-msdt:/id x", "smb://evil/share"] {
            assert_eq!(navigation(&url(s)), Route::Refused, "{s}");
        }
    }

    #[test]
    fn a_log_line_never_carries_the_path_or_the_address() {
        assert_eq!(shown(&url("mailto:someone@example.com")), "mailto:");
        assert_eq!(
            shown(&url("https://drive.google.com/file/d/secret?code=x")),
            "https://drive.google.com"
        );
    }
}
