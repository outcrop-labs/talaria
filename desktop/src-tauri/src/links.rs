//! Where a link that leaves an instance page goes.
//!
//! An instance webview is a browser tab without a browser around it. A
//! `target="_blank"` link or a `window.open` asks the engine for a new window,
//! and with no handler installed wry drops that request, so on every OS the
//! click did nothing. A plain `mailto:` link had nowhere to go on WebKitGTK,
//! which reports only that it cannot show the URL.
//!
//! Where each kind of link goes:
//! - **Another site, or a page of the instance's own** (http or https) opens in
//!   the default browser. The person is signed in to Drive, GitHub, a source
//!   the agent read or a coding-harness sign-in there, and a browser tab has
//!   an address bar. At most one hand-off per webview a second
//!   (`HAND_OFF_GAP`): a script cannot flood the browser with tabs.
//! - **The MCP connect popup** (`CONNECT_POPUP`, `/api/mcp/oauth/start` on the
//!   instance's own origin) is the one in-app window: the engine's own popup,
//!   with the instance's session and `window.opener`. It signs in there and
//!   posts back to the page that opened it. The default browser has no
//!   instance session, so the connect would start signed out. That window has
//!   no address bar and none of the shell's handlers, so nothing else opens in
//!   it.
//! - **`mailto:`, `tel:` and `sms:`** (`OS_SCHEMES`, an allowlist) go to the
//!   OS, and only from a click the person made in the instance's own page:
//!   `OS_LINKS`, the shell's script in the main frame, catches a trusted click
//!   on such a link and asks `desktop_open_link`.
//! - **Everything else is refused and logged**:
//!   - every other scheme (an IDE's, Office's, an installer's);
//!   - an OS link that arrives as a navigation or a new-window request;
//!   - `file:`, `javascript:`, `data:`, `blob:` and `about:` as a new window.
//!
//! **Why only a click.** The navigation and new-window handlers cannot tell a
//! click from a script, or the page from a frame inside it:
//! - WebKitGTK and WKWebView ask about subframe navigations and redirects with
//!   no frame or gesture flag, and WebView2's new-window request reaches wry's
//!   handler without `IsUserInitiated`;
//! - a microsite artifact is agent-written HTML, run in a sandboxed iframe that
//!   may run scripts and open popups (ArtifactEditor.svelte,
//!   ArtifactPublic.svelte).
//!
//! The sandbox is the engine's control, and the shell's handlers sit outside
//! it. So an OS scheme from a handler is refused, whatever it is. The click
//! script runs only in the main frame (initialization scripts do), never in an
//! artifact's frame, and an origin with no grant cannot reach the command.
//! App links such as Zoom's or Teams' have https forms: those go to the default
//! browser, which asks before it hands the link to the app.
//!
//! Neither handler opens anything itself. A popup is the engine's own
//! (`NewWindowResponse::Allow`). A hand-off runs on a blocking worker, because
//! opening waits for the launcher (`xdg-open`, `/usr/bin/open`), and the
//! handlers are called on the main thread (scripts/desktop-threading.mjs).

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use tauri::{Runtime, Url, Webview, webview::NewWindowResponse};

/// What happens to a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// It stays in the app: the navigation goes ahead, or the new window is
    /// the engine's own popup.
    InApp,
    /// The default browser.
    Browser,
    /// Nowhere.
    Refused,
}

/// The schemes the OS gets, and only from `desktop_open_link`. The click
/// script (`OS_LINKS`) names the same three; a test holds them together.
pub(crate) const OS_SCHEMES: &[&str] = &["mailto", "tel", "sms"];

/// The one same-origin path that opens in the app: the MCP connect popup
/// (ui `connectPopup`, api `mcp_oauth_start`).
pub(crate) const CONNECT_POPUP: &str = "/api/mcp/oauth/start";

/// How long after one hand-off to the browser or the OS a webview's next one
/// is dropped.
const HAND_OFF_GAP: Duration = Duration::from_secs(1);

/// Schemes the engine loads itself: pages and their frames, sockets, Tauri's
/// own protocols, the engines' internal pages. Every other scheme in a
/// navigation is refused.
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

/// Injected into every instance webview's main frame
/// (commands.rs::create_instance_webview): a click the person made on a
/// `mailto:`, `tel:` or `sms:` link goes to the OS. Only a trusted click (not
/// `a.click()` from a script), with the primary button, that no handler in
/// the page took. `scripts/desktop-os-links.mjs` runs it in `bun run check`.
pub const OS_LINKS: &str = r#"
  if (window.__TAURI_INTERNALS__) {
    const os = ['mailto:', 'tel:', 'sms:'];
    window.addEventListener('click', (e) => {
      if (!e.isTrusted || e.defaultPrevented || e.button !== 0) return;
      const a = e.composedPath().find((n) => n instanceof Element && n.localName === 'a' && n.hasAttribute('href'));
      if (!a || typeof a.href !== 'string') return;
      let url;
      try { url = new URL(a.href); } catch (_) { return; }
      if (!os.includes(url.protocol)) return;
      e.preventDefault();
      window.__TAURI_INTERNALS__.invoke('desktop_open_link', { url: url.href }).catch(() => {});
    });
  }
"#;

/// A navigation inside an instance page: it loads, or it is refused.
pub(crate) fn navigation(url: &Url) -> Route {
    let scheme = url.scheme();
    if ENGINE.contains(&scheme) {
        Route::InApp
    } else {
        Route::Refused
    }
}

/// A new window that an instance page asked for (`target="_blank"`,
/// `window.open`). `instance` is the instance's registered URL.
pub(crate) fn new_window(instance: &Url, url: &Url) -> Route {
    match url.scheme() {
        "http" | "https" if url.origin() == instance.origin() && url.path() == CONNECT_POPUP => {
            Route::InApp
        }
        "http" | "https" => Route::Browser,
        _ => Route::Refused,
    }
}

/// Whether `desktop_open_link` hands `url` to the OS.
pub(crate) fn os_link(url: &Url) -> bool {
    OS_SCHEMES.contains(&url.scheme())
}

/// What a log line says about a URL: its scheme and host, never its path or
/// query (a mailto: address, an OAuth code, an artifact's slug).
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
                "talaria-desktop: {label}: the MCP connect opens in the app ({})",
                shown(&url)
            );
            NewWindowResponse::Allow
        }
        Route::Browser => {
            hand_off(label, url, "the default browser");
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

/// The navigation half, for `allowed_navigation`: false cancels it. It never
/// opens anything: an OS link reaches the OS only from a click
/// (`desktop_open_link`).
pub(crate) fn allow_navigation(label: &str, url: &Url) -> bool {
    match navigation(url) {
        Route::InApp | Route::Browser => true,
        Route::Refused => {
            let why = if os_link(url) {
                ", which opens only from a click in the page"
            } else {
                ""
            };
            eprintln!(
                "talaria-desktop: {label}: refused a navigation to {}{why}",
                shown(url)
            );
            false
        }
    }
}

/// A `mailto:`, `tel:` or `sms:` link the person clicked in an instance's own
/// page, sent by `OS_LINKS`. The one way to the OS (the module note says why).
/// Only an instance webview asks, and only its own origin holds the grant, so
/// an artifact's frame, which has neither the script nor an origin of its own,
/// cannot.
#[tauri::command]
pub async fn desktop_open_link(webview: Webview, url: String) -> Result<(), String> {
    let label = webview.label().to_string();
    if !label.starts_with("instance-") {
        return Err("only an instance page opens links".into());
    }
    let url: Url = url.parse().map_err(|_| "not a URL".to_string())?;
    if !os_link(&url) {
        eprintln!(
            "talaria-desktop: {label}: refused to open {} (only mailto:, tel: and sms: go to the OS)",
            shown(&url)
        );
        return Err("only mailto:, tel: and sms: links open outside the app".into());
    }
    hand_off(&label, url, "the OS");
    Ok(())
}

/// When each webview last handed a link out.
static LAST_HAND_OFF: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());

/// Whether `label` may hand a link out at `now`: not within `HAND_OFF_GAP` of
/// its last one. A hand-off that goes ahead starts the gap again.
fn take_turn(last: &mut BTreeMap<String, Instant>, label: &str, now: Instant) -> bool {
    if let Some(at) = last.get(label)
        && now.saturating_duration_since(*at) < HAND_OFF_GAP
    {
        return false;
    }
    last.insert(label.to_string(), now);
    true
}

/// Give `url` to the default browser or the OS handler for its scheme, on a
/// blocking worker: `open` waits for the launcher to start or exit, so the
/// handler that asked never waits. A second hand-off from the same webview
/// within `HAND_OFF_GAP` is dropped.
fn hand_off(label: &str, url: Url, to: &'static str) {
    let turn = take_turn(
        &mut LAST_HAND_OFF.lock().unwrap_or_else(PoisonError::into_inner),
        label,
        Instant::now(),
    );
    if !turn {
        eprintln!(
            "talaria-desktop: {label}: dropped {} for {to}: another link went out under a second ago",
            shown(&url)
        );
        return;
    }
    eprintln!("talaria-desktop: {label}: {} goes to {to}", shown(&url));
    let label = label.to_string();
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

    /// Schemes some desktop has a handler for, which no handler here may hand
    /// over: IDEs, Office, Java Web Start, the Windows setup host, installers,
    /// meeting clients, remote shares.
    const UNLISTED: &[&str] = &[
        "vscode://file/etc/passwd",
        "vscode-insiders://x",
        "jnlp://evil.example/app.jnlp",
        "ms-cxh://setsignin",
        "ms-officecmd:{}",
        "ms-msdt:/id PCWDiagnostic",
        "search-ms:query=x&crumb=location:\\\\evil\\share",
        "ms-appinstaller:?source=https://evil/x.appx",
        "steam://run/1",
        "zoommtg://zoom.us/join?confno=1",
        "msteams:/l/meetup-join/x",
        "slack://open",
        "webcal://example.com/cal.ics",
        "xmpp:someone@example.com",
        "smb://evil.example/share",
        "ftp://example.com/file",
        "shell:startup",
        "vbscript:msgbox(1)",
        "view-source:https://example.com",
        "x-unknown-handler:anything",
    ];

    #[test]
    fn another_site_opens_in_the_default_browser() {
        for s in [
            "https://drive.google.com/file/d/abc/view",
            "https://github.com/settings/apps/new",
            "http://example.org/",
            // Same host, other scheme or port: another origin.
            "http://talaria.example.com/",
            "https://talaria.example.com:8443/api/mcp/oauth/start",
            "https://sub.talaria.example.com/",
        ] {
            assert_eq!(opened(s), Route::Browser, "{s}");
        }
    }

    #[test]
    fn only_the_mcp_connect_stays_in_the_app() {
        for s in [
            "https://talaria.example.com/api/mcp/oauth/start?server=x&scope=me",
            "https://talaria.example.com:443/api/mcp/oauth/start?server=x&scope=org",
            "https://TALARIA.example.com/api/mcp/oauth/start",
        ] {
            assert_eq!(opened(s), Route::InApp, "{s}");
        }
        // Every other page of the instance goes to the browser, which has an
        // address bar: a public artifact, a download, a page of the app.
        for s in [
            "https://talaria.example.com/a/public-slug",
            "https://talaria.example.com/api/artifacts/public/x/download",
            "https://talaria.example.com/settings",
            "https://talaria.example.com/api/mcp/oauth/start/extra",
            "https://talaria.example.com/api/mcp/oauth/callback",
        ] {
            assert_eq!(opened(s), Route::Browser, "{s}");
        }
        // A dev instance's port is part of its origin.
        let dev = url("http://127.0.0.1:5302/");
        let start = "http://127.0.0.1:5302/api/mcp/oauth/start?server=x";
        assert_eq!(new_window(&dev, &url(start)), Route::InApp);
        let other_port = "http://127.0.0.1:5303/api/mcp/oauth/start";
        assert_eq!(new_window(&dev, &url(other_port)), Route::Browser);
    }

    #[test]
    fn no_handler_hands_a_link_to_the_os() {
        // Not even the three the OS gets from a click: a handler cannot tell
        // a click from a script, or the page from an artifact's frame.
        for s in [
            "mailto:someone@example.com",
            "tel:+15551234567",
            "sms:+15551234567",
        ] {
            assert_eq!(opened(s), Route::Refused, "{s}");
            assert_eq!(navigation(&url(s)), Route::Refused, "{s}");
            assert!(os_link(&url(s)), "{s}");
        }
        for s in UNLISTED {
            assert_eq!(opened(s), Route::Refused, "{s}");
            assert_eq!(navigation(&url(s)), Route::Refused, "{s}");
        }
    }

    #[test]
    fn the_os_gets_mailto_tel_and_sms_and_nothing_else() {
        for s in [
            "mailto:someone@example.com?subject=hi",
            "MAILTO:someone@example.com",
            "tel:+15551234567",
            "sms:+15551234567",
        ] {
            assert!(os_link(&url(s)), "{s}");
        }
        for s in UNLISTED {
            assert!(!os_link(&url(s)), "{s}");
        }
        for s in [
            "https://example.com",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(!os_link(&url(s)), "{s}");
        }
    }

    #[test]
    fn the_click_script_names_the_os_schemes() {
        let names: Vec<String> = OS_SCHEMES.iter().map(|s| format!("'{s}:'")).collect();
        assert!(
            OS_LINKS.contains(&format!("const os = [{}];", names.join(", "))),
            "OS_LINKS and OS_SCHEMES disagree"
        );
        assert!(OS_LINKS.contains("e.isTrusted"));
        assert!(OS_LINKS.contains("invoke('desktop_open_link'"));
    }

    #[test]
    fn a_local_file_or_a_script_is_never_opened() {
        for s in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<p>x</p>",
            "blob:https://talaria.example.com/1234",
            "about:blank",
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
        assert_eq!(navigation(&url("file:///home/x/a.pdf")), Route::Refused);
    }

    #[test]
    fn a_webview_hands_out_one_link_a_second() {
        let mut last = BTreeMap::new();
        let t0 = Instant::now();
        assert!(take_turn(&mut last, "instance-a", t0));
        // A flood from one page: all dropped inside the gap.
        for ms in [1, 10, 500, 999] {
            assert!(!take_turn(
                &mut last,
                "instance-a",
                t0 + Duration::from_millis(ms)
            ));
        }
        // Another instance is not held up by it.
        assert!(take_turn(
            &mut last,
            "instance-b",
            t0 + Duration::from_millis(5)
        ));
        // A second on, the next goes, and starts the gap again.
        let t1 = t0 + Duration::from_millis(1000);
        assert!(take_turn(&mut last, "instance-a", t1));
        assert!(!take_turn(
            &mut last,
            "instance-a",
            t1 + Duration::from_millis(300)
        ));
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
