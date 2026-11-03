//! A way back when an instance page does not come up (W3).
//!
//! The window has no chrome of its own in the default (themed) titlebar mode:
//! until the instance's UI draws its titlebar and switcher, there is no close,
//! no minimize, no drag and no way back to the launcher. So a page that never
//! loads (a request Cloudflare holds for 100 s while the instance restarts), or
//! that loads an error (Cloudflare's 502 or 524, WebView2's network error page,
//! where even the shell's escape keys do not run), left a person in a window
//! they could only kill. The shell watches every instance page's load instead:
//!
//! - **The watchdog**, every OS. `arm` when a webview is created, and when a
//!   new document commits after a load that settled (a reload, a link to a new
//!   page). `settle` on the page-load Finished event. A load still unsettled
//!   `LOAD_WAIT` later, whose page has not installed the UI's shell hook
//!   (`window.__TALARIA_DESKTOP__`: a page whose `load` waits on a slow image
//!   is up), is `stalled`: on screen, it is the launcher's card "<instance> is
//!   not answering" with Retry, Keep waiting and Back to instances; off screen,
//!   the page is closed and logged. Keep waiting shows it again and waits
//!   `KEEP_WAIT` more.
//! - **A failed load**: a network error, or an HTTP 5xx main-frame answer,
//!   where the platform says so (`page_failure`). Linux: WebKitGTK's
//!   `load-failed` and the main resource's status at commit. Windows:
//!   WebView2's `NavigationCompleted` (`webview2`). macOS: WKWebView tells
//!   only its navigation delegate, which is wry's, and WebKit's Navigation
//!   Timing has no `responseStatus`; so a page that finished without the
//!   Talaria UI in it (an error page, Cloudflare's 502) is asked of the server
//!   again by the shell (`page_status`). A load that fails before it commits
//!   never finishes there, and the watchdog takes it. Each closes the page and
//!   shows the same card, with Retry and Back to instances. Every one is
//!   queued: nothing is done inside the platform's callback.

use std::{collections::HashMap, time::Duration};

/// How long a page may take to load before the shell offers a way back.
/// Cloudflare holds a request to a restarting origin for about 100 s; a
/// Talaria page that is up finishes in a second or two.
pub const LOAD_WAIT: Duration = Duration::from_secs(25);
/// Keep waiting: how much longer, before the card comes back.
pub const KEEP_WAIT: Duration = Duration::from_secs(60);
/// How long the page may take to say whether its UI is up.
pub const HOOK_WAIT: Duration = Duration::from_secs(3);

/// Asked of a page whose load has not finished: has the Talaria UI booted in
/// it? `true` only for a page whose document committed and ran the UI.
pub const HOOK_JS: &str =
    "typeof window.__TALARIA_DESKTOP__ === 'object' && window.__TALARIA_DESKTOP__ !== null";

/// How long the shell's own look at a page's status may take (macOS,
/// `page_status`).
#[cfg(any(target_os = "macos", test))]
pub const STATUS_WAIT: Duration = Duration::from_secs(10);

/// Where an instance page's load stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Load {
    /// Loading, with the watchdog's generation.
    Armed(u64),
    /// Past its wait and on the card (or closed); a Finished now settles it.
    Stalled(u64),
    /// It failed (`page_failure`): whatever finishes after this changes nothing.
    Failed,
}

/// Every instance page's load, by local id (ShellState.loads).
#[derive(Debug, Default)]
pub struct Loads {
    next: u64,
    pages: HashMap<String, Load>,
}

/// What a Finished did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// Nothing was waiting (a load that settled already, or a page that failed).
    Nothing,
    /// A load the watchdog was waiting on.
    InTime,
    /// A load that had already stalled: its card may be up.
    AfterStall,
}

impl Loads {
    /// A load starts (a document committed after a load that settled): the
    /// generation the watchdog waits on. A load still being watched keeps its
    /// state: an armed one its deadline, so the commit does not push it back,
    /// and a stalled one its stall, so its Finished still reopens it from the
    /// card.
    pub fn arm(&mut self, id: &str) -> Option<u64> {
        if self.pages.contains_key(id) {
            return None;
        }
        Some(self.rearm(id))
    }

    /// A new deadline whatever came before (a new webview; Keep waiting).
    pub fn rearm(&mut self, id: &str) -> u64 {
        self.next += 1;
        self.pages.insert(id.to_string(), Load::Armed(self.next));
        self.next
    }

    /// The page finished loading.
    pub fn settle(&mut self, id: &str) -> Settled {
        match self.pages.get(id) {
            Some(Load::Armed(_)) => {
                self.pages.remove(id);
                Settled::InTime
            }
            Some(Load::Stalled(_)) => {
                self.pages.remove(id);
                Settled::AfterStall
            }
            Some(Load::Failed) | None => Settled::Nothing,
        }
    }

    /// The watchdog's turn: is `generation` still the load it armed for? If
    /// so it has stalled now, and true.
    pub fn stall(&mut self, id: &str, generation: u64) -> bool {
        if self.pages.get(id) == Some(&Load::Armed(generation)) {
            self.pages.insert(id.to_string(), Load::Stalled(generation));
            true
        } else {
            false
        }
    }

    /// The watchdog woke: is `generation` still the unsettled load it armed for?
    pub fn armed(&self, id: &str, generation: u64) -> bool {
        self.pages.get(id) == Some(&Load::Armed(generation))
    }

    /// The page is up after all (its UI hook answered): nothing to wait for.
    pub fn up(&mut self, id: &str, generation: u64) {
        if self.pages.get(id) == Some(&Load::Armed(generation)) {
            self.pages.remove(id);
        }
    }

    /// Is this page on the stall card (Keep waiting re-arms it)?
    pub fn stalled(&self, id: &str) -> bool {
        matches!(self.pages.get(id), Some(Load::Stalled(_)))
    }

    /// The page failed. Synchronous in the platform's callback, before the
    /// work is queued, so a Finished that the same navigation raised first
    /// cannot read as a page that came up.
    pub fn fail(&mut self, id: &str) {
        self.pages.insert(id.to_string(), Load::Failed);
    }

    pub fn failed(&self, id: &str) -> bool {
        self.pages.get(id) == Some(&Load::Failed)
    }

    /// The webview is gone (closed, suspended, removed): nothing to watch.
    pub fn forget(&mut self, id: &str) {
        self.pages.remove(id);
    }
}

/// A main-frame load that failed, in the words the card shows after the
/// instance's name: an HTTP status of 500 or more (`status`), or a network
/// failure the platform named (`network`). None: the page is the page (a
/// 404 is the instance's own answer, and its UI shows it).
pub fn page_failure(status: Option<u16>, network: Option<&str>) -> Option<String> {
    if let Some(code) = status.filter(|c| *c >= 500) {
        let what = match code {
            500 => " (Internal server error)",
            502 => " (Bad gateway)",
            503 => " (Service unavailable)",
            504 => " (Gateway timeout)",
            520 => " (Cloudflare: unknown error)",
            521 => " (Cloudflare: the web server is down)",
            522 => " (Cloudflare: the connection timed out)",
            523 => " (Cloudflare: the origin is unreachable)",
            524 => " (Cloudflare: a timeout occurred)",
            530 => " (Cloudflare: the tunnel is down)",
            _ => "",
        };
        return Some(format!("answered {code}{what}: it may be restarting"));
    }
    network.map(|why| format!("did not load: {why}"))
}

/// The card's sentence for a load that stalled.
pub fn stalled_message(instance: &str) -> String {
    format!(
        "{instance} is not answering: its page has not loaded in {} seconds. If it is restarting, it should be back in a minute.",
        LOAD_WAIT.as_secs()
    )
}

/// What the server answers for `url` now, by the shell's own HEAD request
/// (macOS, for a page that finished without the Talaria UI in it). No
/// cookies, the shell's own proxy and trust, like the launcher's probe: an
/// instance root answers 200 to anyone, and Cloudflare's 502 is everyone's.
/// None: no answer within `STATUS_WAIT`.
#[cfg(any(target_os = "macos", test))]
pub async fn page_status(client: &reqwest::Client, url: &str) -> Option<u16> {
    let response = tokio::time::timeout(STATUS_WAIT, client.head(url).send())
        .await
        .ok()?
        .ok()?;
    Some(response.status().as_u16())
}

/// WebView2's `NavigationCompleted`, judged (`windows::navigation_completed`
/// reads the three facts). `http` is `HttpStatusCode`, 0 when the load got no
/// HTTP answer. A navigation another one replaced, or the page stopped, is
/// cancelled, not failed. WebView2 also calls a 4xx unsuccessful, but that is
/// a page the instance answered: only a 5xx, or no answer at all, is a
/// failure (the network error page, where no init script runs).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn webview2_failure(success: bool, cancelled: bool, error: &str, http: i32) -> Option<String> {
    let status = u16::try_from(http).ok().filter(|c| *c > 0);
    if status.is_some_and(|c| c >= 500) {
        return page_failure(status, None);
    }
    if success || cancelled || status.is_some() {
        return None;
    }
    page_failure(None, Some(error))
}

/// WebView2's error status, in words (COREWEBVIEW2_WEB_ERROR_STATUS; the
/// Windows build asserts these numbers against webview2-com's constants).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn webview2_error_words(status: i32) -> &'static str {
    match status {
        1 => "its certificate is for another name",
        2 => "its certificate has expired",
        3 => "a client certificate was refused",
        4 => "its certificate was revoked",
        5 => "its certificate is not trusted",
        6 => "the server is unreachable",
        7 => "it did not answer in time",
        8 => "its answer was not valid HTTP",
        9 => "the connection was aborted",
        10 => "the connection was reset",
        11 => "the network is disconnected",
        12 => "nothing is accepting connections there",
        13 => "the name does not resolve to an address",
        15 => "a redirect failed",
        16 => "an unexpected error",
        17 => "it asked for credentials",
        18 => "the proxy asked for credentials",
        _ => "the page could not be reached",
    }
}

/// COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED: a navigation another
/// one replaced, or the page stopped.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_CANCELLED: i32 = 14;
/// COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT and _HOST_NAME_NOT_RESOLVED,
/// for the Windows build's assert.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_CANNOT_CONNECT: i32 = 12;
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_NOT_RESOLVED: i32 = 13;

/// WebView2's main-frame `NavigationCompleted`, read for `webview2_failure`
/// (W3). wry hands the same event to the page-load handler as Finished and
/// keeps the rest; this is a second subscription, on the same CoreWebView2.
#[cfg(windows)]
pub mod webview2 {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT,
            COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED, ICoreWebView2,
            ICoreWebView2NavigationCompletedEventArgs, ICoreWebView2NavigationCompletedEventArgs2,
        },
        NavigationCompletedEventHandler,
    };
    use windows_core::{BOOL, Interface};

    // The words table above is by number: hold it to webview2-com's own.
    const _: () =
        assert!(COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED.0 == super::WEBVIEW2_CANCELLED);
    const _: () =
        assert!(COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT.0 == super::WEBVIEW2_CANNOT_CONNECT);
    const _: () = assert!(
        COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED.0 == super::WEBVIEW2_NOT_RESOLVED
    );

    /// Hand `failed` the failure of every main-frame navigation of `core` that
    /// failed, in words. It is called inside WebView2's event handler, so it
    /// must only record and queue (commands::navigation_failed).
    pub fn on_navigation_completed(
        core: &ICoreWebView2,
        failed: impl Fn(String) + 'static,
    ) -> windows_core::Result<()> {
        let handler = NavigationCompletedEventHandler::create(Box::new(move |_, args| {
            if let Some(why) = args.as_ref().and_then(judge) {
                failed(why);
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: a live CoreWebView2 (wry's own, alive as long as the
        // webview), a handler COM object it now holds a reference to, and a
        // token to write.
        unsafe { core.add_NavigationCompleted(&handler, &mut token) }
    }

    fn judge(args: &ICoreWebView2NavigationCompletedEventArgs) -> Option<String> {
        let mut success = BOOL::default();
        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
        // SAFETY: the event's own args, read for the duration of the handler.
        unsafe {
            args.IsSuccess(&mut success).ok()?;
            args.WebErrorStatus(&mut status).ok()?;
        }
        // `HttpStatusCode` is on the second version of the args (WebView2
        // runtime 1.0.1185, 2022); an older runtime reads as no status.
        let http = args
            .cast::<ICoreWebView2NavigationCompletedEventArgs2>()
            .ok()
            .and_then(|args| {
                let mut code = 0i32;
                // SAFETY: as above.
                unsafe { args.HttpStatusCode(&mut code) }.ok().map(|_| code)
            })
            .unwrap_or(0);
        super::webview2_failure(
            success.as_bool(),
            status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
            super::webview2_error_words(status.0),
            http,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_that_finishes_in_time_never_stalls() {
        let mut loads = Loads::default();
        let generation = loads.arm("a").unwrap();
        assert_eq!(loads.settle("a"), Settled::InTime);
        assert!(!loads.stall("a", generation));
        // A second Finished (a frame, a repeat) changes nothing.
        assert_eq!(loads.settle("a"), Settled::Nothing);
    }

    #[test]
    fn a_load_that_does_not_finish_stalls_once() {
        let mut loads = Loads::default();
        let generation = loads.arm("a").unwrap();
        assert!(loads.stall("a", generation));
        assert!(loads.stalled("a"));
        assert!(!loads.stall("a", generation));
        // It finishes while the card is up.
        assert_eq!(loads.settle("a"), Settled::AfterStall);
        assert!(!loads.stalled("a"));
    }

    #[test]
    fn a_commit_does_not_push_the_deadline_back_and_keep_waiting_does() {
        let mut loads = Loads::default();
        let first = loads.arm("a").unwrap();
        // The document commits: still the first deadline.
        assert_eq!(loads.arm("a"), None);
        assert!(loads.stall("a", first));
        // Keep waiting: a new deadline, and the old one's timer is stale.
        let second = loads.rearm("a");
        assert_ne!(first, second);
        assert!(!loads.stall("a", first));
        assert!(loads.stall("a", second));
    }

    #[test]
    fn the_commit_of_a_stalled_load_leaves_it_stalled() {
        // The held request is answered while the card is up: the document
        // commits (Started), then finishes. The Finished must read as "after
        // the stall", or the shell never opens it from the card.
        let mut loads = Loads::default();
        let generation = loads.rearm("a");
        assert!(loads.stall("a", generation));
        assert_eq!(loads.arm("a"), None);
        assert!(loads.stalled("a"));
        assert_eq!(loads.settle("a"), Settled::AfterStall);
        // And a commit after the page failed leaves it failed.
        loads.rearm("b");
        loads.fail("b");
        assert_eq!(loads.arm("b"), None);
        assert!(loads.failed("b"));
    }

    #[test]
    fn a_page_whose_ui_is_up_is_not_a_stall() {
        let mut loads = Loads::default();
        let generation = loads.arm("a").unwrap();
        loads.up("a", generation);
        assert!(!loads.stall("a", generation));
        // A stale answer about an older load leaves a newer one armed.
        let newer = loads.arm("a").unwrap();
        loads.up("a", generation);
        assert!(loads.stall("a", newer));
    }

    #[test]
    fn a_failed_page_is_not_brought_back_by_its_own_finished() {
        let mut loads = Loads::default();
        let generation = loads.arm("a").unwrap();
        loads.fail("a");
        assert!(loads.failed("a"));
        assert_eq!(loads.settle("a"), Settled::Nothing);
        assert!(!loads.stall("a", generation));
        // A new webview starts over.
        loads.forget("a");
        assert!(loads.arm("a").is_some());
        assert!(!loads.failed("a"));
    }

    #[test]
    fn each_page_is_watched_on_its_own() {
        let mut loads = Loads::default();
        let a = loads.arm("a").unwrap();
        let b = loads.arm("b").unwrap();
        assert_eq!(loads.settle("a"), Settled::InTime);
        assert!(!loads.stall("a", a));
        assert!(loads.stall("b", b));
    }

    #[test]
    fn a_5xx_or_no_answer_is_a_failure_and_a_4xx_is_not() {
        assert_eq!(
            page_failure(Some(502), None).unwrap(),
            "answered 502 (Bad gateway): it may be restarting"
        );
        assert_eq!(
            page_failure(Some(524), None).unwrap(),
            "answered 524 (Cloudflare: a timeout occurred): it may be restarting"
        );
        assert_eq!(
            page_failure(Some(599), None).unwrap(),
            "answered 599: it may be restarting"
        );
        assert_eq!(page_failure(Some(200), None), None);
        assert_eq!(page_failure(Some(404), None), None);
        assert_eq!(page_failure(None, None), None);
        assert_eq!(
            page_failure(Some(200), Some("refused")).unwrap(),
            "did not load: refused"
        );
    }

    #[test]
    fn webview2_navigation_completed_is_judged_like_the_other_engines() {
        // Cloudflare's 502 and 524 pages: unsuccessful, with a status.
        assert!(
            webview2_failure(false, false, "x", 502)
                .unwrap()
                .contains("502 (Bad gateway)")
        );
        assert!(
            webview2_failure(true, false, "x", 524)
                .unwrap()
                .contains("524")
        );
        // A 404 the instance answered, a page that loaded: not failures.
        assert_eq!(webview2_failure(false, false, "x", 404), None);
        assert_eq!(webview2_failure(true, false, "x", 200), None);
        // The network error page: no HTTP answer at all.
        assert_eq!(
            webview2_failure(
                false,
                false,
                webview2_error_words(WEBVIEW2_CANNOT_CONNECT),
                0
            )
            .unwrap(),
            "did not load: nothing is accepting connections there"
        );
        // Replaced by another navigation, or stopped: not a failure.
        assert_eq!(
            webview2_failure(false, true, webview2_error_words(WEBVIEW2_CANCELLED), 0),
            None
        );
        // A negative or zero status is no status.
        assert!(webview2_failure(false, false, "x", -1).is_some());
        assert_eq!(
            webview2_error_words(WEBVIEW2_NOT_RESOLVED),
            "the name does not resolve to an address"
        );
        assert_eq!(webview2_error_words(99), "the page could not be reached");
    }

    #[test]
    fn the_stall_card_says_how_long_and_what_it_may_be() {
        let said = stalled_message("Fake A");
        assert!(
            said.starts_with("Fake A is not answering: its page has not loaded in 25 seconds.")
        );
        assert!(LOAD_WAIT >= Duration::from_secs(20) && LOAD_WAIT <= Duration::from_secs(30));
        assert!(KEEP_WAIT > LOAD_WAIT);
    }

    #[test]
    fn the_hook_question_names_the_ui_hook() {
        assert!(HOOK_JS.contains("__TALARIA_DESKTOP__"));
        assert!(
            !HOOK_JS.contains(';'),
            "one expression, so eval answers its value"
        );
    }

    /// One HTTP answer on a local port, for `page_status`.
    fn answering(status_line: &'static str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status_line}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
            }
        });
        url
    }

    #[tokio::test]
    async fn the_shell_reads_the_status_a_server_answers_now() {
        let client = reqwest::Client::new();
        assert_eq!(
            page_status(&client, &answering("502 Bad Gateway")).await,
            Some(502)
        );
        assert_eq!(page_status(&client, &answering("200 OK")).await, Some(200));
        let said = page_failure(
            page_status(&client, &answering("524 Origin Time-out")).await,
            None,
        );
        assert!(said.unwrap().contains("524"));
        // Nothing there: no status, so no card from this (the watchdog and the
        // engine have the other cases).
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/", l.local_addr().unwrap())
        };
        assert_eq!(page_status(&client, &closed).await, None);
    }
}
