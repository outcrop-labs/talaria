//! A way back when an instance page does not come up (W3).
//!
//! The window has no chrome of its own in the default (themed) titlebar mode:
//! the instance's UI draws the titlebar and the switcher. So a page that never
//! loads (a request Cloudflare holds for 100 s while the instance restarts), or
//! that loads something other than Talaria (Cloudflare's 502, 524 or 403,
//! WebView2's network error page, a mis-routed 404), left a person in a window
//! they could only kill. Three rules, the same on every OS:
//!
//! 1. **A page with the Talaria UI up is never interrupted.** Up is the UI's
//!    shell hook (`window.__TALARIA_DESKTOP__`), or, for a Talaria from before
//!    the hook, the page's titlebar asking the shell for its settings
//!    (`get_desktop_settings`, which every UI's titlebar does as it mounts:
//!    `ui_seen`). A failed download, an aborted navigation or a 5xx that does
//!    not replace the page changes nothing.
//! 2. **A page on the instance's own origin that is not Talaria is the card.**
//!    A main-frame page there that finished without the UI coming up
//!    `UI_GRACE` later — an HTML page, an engine's error page, or anything the
//!    platform said failed — is the launcher's card, with the platform's
//!    words when it had any (`note`: a 5xx, a 4xx, a network failure). A page
//!    that never finishes is the watchdog's: `LOAD_WAIT` after a webview is
//!    made or a document commits, a page still loading without the UI is
//!    "<instance> is not answering". A file the person opened (an image, a
//!    PDF, JSON: not HTML, and nothing failed) is left alone.
//! 3. **Pages on other origins are never judged.** A sign-in provider, a
//!    Cloudflare Access login: not Talaria, and not the shell's to call
//!    broken.
//!
//! The platform's words: Linux, WebKitGTK's `load-failed` and the main
//! resource's status at commit; Windows, WebView2's `NavigationCompleted`
//! (`webview2`); macOS has none outside wry's navigation delegate, and rule
//! 2 needs none. Each callback only records; the decision is a main-thread
//! turn of its own.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

/// How long a page may take to load before the shell offers a way back.
/// Cloudflare holds a request to a restarting origin for about 100 s; a
/// Talaria page that is up finishes in a second or two.
pub const LOAD_WAIT: Duration = Duration::from_secs(25);
/// Keep waiting: how much longer, before the card comes back.
pub const KEEP_WAIT: Duration = Duration::from_secs(60);
/// How long the page may take to answer `PAGE_JS`.
pub const HOOK_WAIT: Duration = Duration::from_secs(3);
/// After a page finished, how long its UI has to come up before it is
/// judged. Both of the UI's signals come before `load` in every version (the
/// hook is set before the app mounts, the titlebar asks as it mounts).
pub const UI_GRACE: Duration = Duration::from_secs(3);

/// Asked of a page: what is it? `ui` (the Talaria UI's hook), `html` (an
/// HTML page without it: an error page, a gate, another app), `other` (a
/// file shown in the window). One expression, so eval answers its value.
pub const PAGE_JS: &str = "(() => { try { const d = window.__TALARIA_DESKTOP__; if (typeof d === 'object' && d !== null) return 'ui'; return document.contentType === 'text/html' ? 'html' : 'other'; } catch (e) { return 'html'; } })()";

/// What a page is, from `PAGE_JS` (or the titlebar's call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Ui,
    Html,
    Other,
    /// No answer: an engine error page where scripts do not run, a page
    /// too busy to answer in `HOOK_WAIT`.
    Silent,
}

/// `PAGE_JS`'s answer as the engine hands it back: a JSON string, or None.
pub fn page_kind(answer: Option<&str>) -> Page {
    match answer.map(|a| a.trim().trim_matches('"')) {
        Some("ui") => Page::Ui,
        Some("html") => Page::Html,
        Some("other") => Page::Other,
        _ => Page::Silent,
    }
}

/// What the shell does with a page on the instance's own origin that
/// finished (rule 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The Talaria UI is up.
    Up,
    /// A file the person opened: not the shell's business.
    Leave,
    /// Not Talaria: the card, with these words after the instance's name.
    Fail(String),
}

/// The words when the platform had none: a page answered, and it is not
/// Talaria.
pub const NOT_TALARIA: &str = "answered with a page that is not Talaria: it may be restarting, or something in front of it (a sign-in gate, a firewall) answered instead";

pub fn verdict(ui_seen: bool, page: Page, noted: Option<&str>) -> Verdict {
    if ui_seen || page == Page::Ui {
        return Verdict::Up;
    }
    if page == Page::Other && noted.is_none() {
        return Verdict::Leave;
    }
    Verdict::Fail(noted.unwrap_or(NOT_TALARIA).to_string())
}

/// Where an instance page's load stands, for the watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Watch {
    /// Loading, with the watchdog's generation.
    Armed(u64),
    /// Past its wait and on the card; a Finished now is judged for a reopen.
    Stalled(u64),
}

/// Every instance page's load, by local id (ShellState.loads).
#[derive(Debug, Default)]
pub struct Loads {
    next: u64,
    watch: HashMap<String, Watch>,
    /// Each webview's document, counted at every commit: a judgement made
    /// for one document is dropped if another has committed since.
    docs: HashMap<String, u64>,
    /// The current document's UI asked the shell for something (rule 1).
    ui: HashSet<String>,
    /// The platform's words on the current document's load (rule 2).
    noted: HashMap<String, String>,
    /// Failed loads in a row of a page loaded hidden at launch.
    prewarm: HashMap<String, u32>,
}

/// What a Finished did to the watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    /// Nothing was watching it.
    Nothing,
    /// A load the watchdog was waiting on.
    InTime,
    /// A load that had stalled: its card may be up.
    AfterStall,
}

impl Loads {
    fn arm_new(&mut self, id: &str) -> u64 {
        self.next += 1;
        self.watch.insert(id.to_string(), Watch::Armed(self.next));
        self.next
    }

    fn new_document(&mut self, id: &str) {
        *self.docs.entry(id.to_string()).or_default() += 1;
        self.ui.remove(id);
        self.noted.remove(id);
    }

    /// A webview is made for the instance: a new document, watched.
    pub fn created(&mut self, id: &str) -> u64 {
        self.new_document(id);
        self.arm_new(id)
    }

    /// A document committed. On the instance's own origin it is watched,
    /// unless a load is still being watched (the webview's first, which keeps
    /// its deadline; a stalled one, which keeps its stall). On another origin
    /// it is not watched at all (rule 3). The generation to wait on, if armed.
    pub fn committed(&mut self, id: &str, own_origin: bool) -> Option<u64> {
        self.new_document(id);
        if !own_origin {
            self.watch.remove(id);
            return None;
        }
        if self.watch.contains_key(id) {
            return None;
        }
        Some(self.arm_new(id))
    }

    /// Keep waiting: a new deadline for a page on the card.
    pub fn rearm(&mut self, id: &str) -> u64 {
        self.arm_new(id)
    }

    pub fn doc(&self, id: &str) -> u64 {
        self.docs.get(id).copied().unwrap_or(0)
    }

    /// The page's UI asked the shell for something (rule 1).
    pub fn ui_seen(&mut self, id: &str) {
        self.ui.insert(id.to_string());
    }

    pub fn ui(&self, id: &str) -> bool {
        self.ui.contains(id)
    }

    /// The platform's words on the current document (a status, a network
    /// failure), for the card if it comes to one.
    pub fn note(&mut self, id: &str, what: String) {
        self.noted.insert(id.to_string(), what);
    }

    pub fn noted(&self, id: &str) -> Option<&str> {
        self.noted.get(id).map(String::as_str)
    }

    /// The page finished loading.
    pub fn settle(&mut self, id: &str) -> Settled {
        match self.watch.remove(id) {
            Some(Watch::Armed(_)) => Settled::InTime,
            Some(Watch::Stalled(_)) => Settled::AfterStall,
            None => Settled::Nothing,
        }
    }

    /// The watchdog woke: is `generation` still the unsettled load it armed for?
    pub fn armed(&self, id: &str, generation: u64) -> bool {
        self.watch.get(id) == Some(&Watch::Armed(generation))
    }

    /// The watchdog's turn: if `generation` is still armed, it has stalled now.
    pub fn stall(&mut self, id: &str, generation: u64) -> bool {
        if self.armed(id, generation) {
            self.watch
                .insert(id.to_string(), Watch::Stalled(generation));
            true
        } else {
            false
        }
    }

    /// A stalled page finished as something that is neither Talaria nor a
    /// failure (a file): it stays on the card, and Keep waiting still re-arms.
    pub fn restall(&mut self, id: &str) {
        self.next += 1;
        self.watch.insert(id.to_string(), Watch::Stalled(self.next));
    }

    /// The page's UI is up after all: nothing to wait for.
    pub fn up(&mut self, id: &str, generation: u64) {
        if self.armed(id, generation) {
            self.watch.remove(id);
        }
    }

    pub fn stalled(&self, id: &str) -> bool {
        matches!(self.watch.get(id), Some(Watch::Stalled(_)))
    }

    /// The pages on the card, still loading behind it.
    pub fn stalled_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .watch
            .iter()
            .filter(|(_, w)| matches!(w, Watch::Stalled(_)))
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// The webview is gone (closed, suspended, removed). Its prewarm count
    /// stays: it is about the instance, not the page.
    pub fn forget(&mut self, id: &str) {
        self.watch.remove(id);
        self.ui.remove(id);
        self.noted.remove(id);
    }

    /// A page loaded hidden at launch did not come up: the number of times
    /// in a row now (`prewarm_backoff`).
    pub fn prewarm_failed(&mut self, id: &str) -> u32 {
        let tries = self.prewarm.entry(id.to_string()).or_default();
        *tries += 1;
        *tries
    }

    /// It came up: the next failure starts the backoff over.
    pub fn prewarm_ok(&mut self, id: &str) {
        self.prewarm.remove(id);
    }
}

/// How long after the `tries`th failure in a row a page loaded hidden at
/// launch is loaded again, so its instance's notifications are not lost for
/// the session. After the fifth, it waits for the person, or the next launch.
pub fn prewarm_backoff(tries: u32) -> Option<Duration> {
    let minutes = match tries {
        1 => 1,
        2 => 2,
        3 => 5,
        4 => 15,
        5 => 30,
        _ => return None,
    };
    Some(Duration::from_secs(minutes * 60))
}

/// The platform's words for a main-frame load: an HTTP status of 400 or
/// more (`status`), or a network failure it named (`network`). None: nothing
/// to say (a 2xx or 3xx answer).
pub fn page_reason(status: Option<u16>, network: Option<&str>) -> Option<String> {
    if let Some(code) = status.filter(|c| *c >= 400) {
        let what = match code {
            400 => " (Bad request)",
            401 => " (Unauthorized)",
            403 => " (Forbidden)",
            404 => " (Not found)",
            429 => " (Too many requests)",
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
        let after = if code >= 500 {
            ": it may be restarting"
        } else {
            ""
        };
        return Some(format!("answered {code}{what}{after}"));
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

/// Is `page` on the same origin as `instance` (rule 3)? Only http(s).
pub fn own_origin(instance: &str, page: &str) -> bool {
    match (url::Url::parse(instance), url::Url::parse(page)) {
        (Ok(base), Ok(page)) => {
            matches!(page.scheme(), "http" | "https") && base.origin() == page.origin()
        }
        _ => false,
    }
}

/// Is `page` an http(s) page on another origin than `instance`'s (rule 3)?
/// A webview whose first load failed shows no http(s) page at all
/// (`about:blank`, nothing): that is still the instance's load.
pub fn elsewhere(instance: &str, page: &str) -> bool {
    (page.starts_with("http://") || page.starts_with("https://")) && !own_origin(instance, page)
}

/// WebView2's `NavigationCompleted`, in words (`webview2::judge` reads the
/// facts). `http` is `HttpStatusCode`, 0 when the load got no HTTP answer. A
/// navigation another one replaced, the page stopped, or one that became a
/// download (`ended`: OPERATION_CANCELED, CONNECTION_ABORTED) says nothing,
/// and neither does a success.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn webview2_reason(success: bool, ended: bool, error: &str, http: i32) -> Option<String> {
    let status = u16::try_from(http).ok().filter(|c| *c > 0);
    if status.is_some_and(|c| c >= 400) {
        return page_reason(status, None);
    }
    if success || ended || status.is_some() {
        return None;
    }
    page_reason(None, Some(error))
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

/// COREWEBVIEW2_WEB_ERROR_STATUS values the judge names (asserted against
/// webview2-com on Windows): replaced or stopped, a navigation that became a
/// download, and two for the tests.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_CANCELLED: i32 = 14;
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_ABORTED: i32 = 9;
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_CANNOT_CONNECT: i32 = 12;
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_NOT_RESOLVED: i32 = 13;

/// WebView2's main-frame `NavigationCompleted`, read for `webview2_reason`
/// (W3). wry hands the same event to the page-load handler as Finished; this
/// is a second subscription, on the same CoreWebView2, that only records.
#[cfg(windows)]
pub mod webview2 {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT,
            COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED,
            COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED,
            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED, ICoreWebView2,
            ICoreWebView2NavigationCompletedEventArgs, ICoreWebView2NavigationCompletedEventArgs2,
        },
        NavigationCompletedEventHandler, take_pwstr,
    };
    use windows_core::{BOOL, Interface, PWSTR};

    // The words table above is by number: hold it to webview2-com's own.
    const _: () =
        assert!(COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED.0 == super::WEBVIEW2_CANCELLED);
    const _: () =
        assert!(COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED.0 == super::WEBVIEW2_ABORTED);
    const _: () =
        assert!(COREWEBVIEW2_WEB_ERROR_STATUS_CANNOT_CONNECT.0 == super::WEBVIEW2_CANNOT_CONNECT);
    const _: () = assert!(
        COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED.0 == super::WEBVIEW2_NOT_RESOLVED
    );

    /// Hand `noted` the URL and the words of every http(s) main-frame
    /// navigation of `core` that the platform has words for. It is called
    /// inside WebView2's event handler, so it must only record
    /// (commands::navigation_noted).
    pub fn on_navigation_completed(
        core: &ICoreWebView2,
        noted: impl Fn(String, String) + 'static,
    ) -> windows_core::Result<()> {
        let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            let Some(url) = source(&sender) else {
                return Ok(());
            };
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Ok(());
            }
            if let Some(what) = judge(&args) {
                noted(url, what);
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: a live CoreWebView2 (wry's own, alive as long as the
        // webview), a handler COM object it now holds a reference to, and a
        // token to write. The handler is released with the CoreWebView2.
        unsafe { core.add_NavigationCompleted(&handler, &mut token) }
    }

    fn source(core: &ICoreWebView2) -> Option<String> {
        let mut uri = PWSTR::null();
        // SAFETY: the sender of the event, read during the handler; the
        // string is ours to free, which take_pwstr does.
        unsafe { core.Source(&mut uri) }.ok()?;
        Some(take_pwstr(uri))
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
        let ended = status == COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED
            || status == COREWEBVIEW2_WEB_ERROR_STATUS_CONNECTION_ABORTED;
        super::webview2_reason(
            success.as_bool(),
            ended,
            super::webview2_error_words(status.0),
            http,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Rule 1: a page with the UI up is never interrupted.
    #[test]
    fn a_page_with_the_ui_up_is_never_interrupted() {
        // Whatever the platform said about the load (a download's 5xx, an
        // aborted navigation), the UI being up wins.
        for noted in [
            None,
            Some("answered 500 (Internal server error): it may be restarting"),
        ] {
            assert_eq!(verdict(false, Page::Ui, noted), Verdict::Up);
            // A Talaria from before the hook: its titlebar asked the shell.
            assert_eq!(verdict(true, Page::Html, noted), Verdict::Up);
            assert_eq!(verdict(true, Page::Silent, noted), Verdict::Up);
        }
    }

    // Rule 2: a page on the instance's own origin that is not Talaria.
    #[test]
    fn a_page_on_its_own_origin_that_is_not_talaria_is_the_card() {
        let cf524 = "answered 524 (Cloudflare: a timeout occurred): it may be restarting";
        // Cloudflare's 524 after a stall, its 403, a 404: HTML, no UI.
        assert_eq!(
            verdict(false, Page::Html, Some(cf524)),
            Verdict::Fail(cf524.into())
        );
        assert_eq!(
            verdict(false, Page::Html, None),
            Verdict::Fail(NOT_TALARIA.into())
        );
        // An engine's error page, where no script answers.
        assert_eq!(
            verdict(false, Page::Silent, Some("did not load: refused")),
            Verdict::Fail("did not load: refused".into())
        );
        assert_eq!(
            verdict(false, Page::Silent, None),
            Verdict::Fail(NOT_TALARIA.into())
        );
        // A failed answer that is not HTML (a 502 in plain text) is a failure.
        assert!(matches!(
            verdict(
                false,
                Page::Other,
                Some("answered 502 (Bad gateway): it may be restarting")
            ),
            Verdict::Fail(_)
        ));
        // A file the person opened, that nothing said failed: left alone.
        assert_eq!(verdict(false, Page::Other, None), Verdict::Leave);
    }

    // Rule 3: pages on other origins are never judged.
    #[test]
    fn only_the_instances_own_origin_is_judged() {
        let base = "https://acme.talaria.example";
        assert!(own_origin(base, "https://acme.talaria.example/comms?x=1"));
        assert!(own_origin(
            "http://127.0.0.1:47393",
            "http://127.0.0.1:47393/"
        ));
        for other in [
            "https://accounts.google.com/o/oauth2/auth?client_id=x",
            "https://github.com/login/oauth/authorize",
            "https://acme.cloudflareaccess.com/cdn-cgi/access/login",
            "http://acme.talaria.example/",
            "http://127.0.0.1:47394/",
            "about:blank",
            "chrome-error://chromewebdata/",
            "file:///home/me/x.pdf",
        ] {
            assert!(!own_origin(base, other), "{other}");
        }
        // The watchdog skips only a real page elsewhere: a webview whose first
        // load failed (no http(s) page) is still the instance's.
        assert!(loading_elsewhere_cases(base));
        // A commit on another origin is not watched, and drops a watch.
        let mut loads = Loads::default();
        loads.created("a");
        assert_eq!(loads.committed("a", false), None);
        assert_eq!(loads.settle("a"), Settled::Nothing);
    }

    fn loading_elsewhere_cases(base: &str) -> bool {
        elsewhere(base, "https://accounts.google.com/x")
            && elsewhere(base, "http://acme.talaria.example/")
            && !elsewhere(base, "https://acme.talaria.example/home")
            && !elsewhere(base, "about:blank")
            && !elsewhere(base, "")
            && !elsewhere(base, "chrome-error://chromewebdata/")
    }

    #[test]
    fn the_page_answers_are_read_as_the_engines_hand_them_back() {
        assert_eq!(page_kind(Some("\"ui\"")), Page::Ui);
        assert_eq!(page_kind(Some("\"html\"")), Page::Html);
        assert_eq!(page_kind(Some(" \"other\" ")), Page::Other);
        assert_eq!(page_kind(Some("null")), Page::Silent);
        assert_eq!(page_kind(Some("")), Page::Silent);
        assert_eq!(page_kind(None), Page::Silent);
        assert!(PAGE_JS.contains("__TALARIA_DESKTOP__") && PAGE_JS.contains("try"));
        assert!(!PAGE_JS.trim_end().ends_with(';'));
    }

    #[test]
    fn a_load_that_finishes_in_time_never_stalls() {
        let mut loads = Loads::default();
        let generation = loads.created("a");
        assert_eq!(loads.settle("a"), Settled::InTime);
        assert!(!loads.stall("a", generation));
        assert_eq!(loads.settle("a"), Settled::Nothing);
    }

    #[test]
    fn a_load_that_does_not_finish_stalls_once() {
        let mut loads = Loads::default();
        let generation = loads.created("a");
        assert!(loads.stall("a", generation));
        assert!(loads.stalled("a"));
        assert!(!loads.stall("a", generation));
        assert_eq!(loads.stalled_ids(), vec!["a".to_string()]);
        assert_eq!(loads.settle("a"), Settled::AfterStall);
        assert!(!loads.stalled("a"));
    }

    #[test]
    fn a_commit_keeps_the_watch_it_finds() {
        let mut loads = Loads::default();
        let first = loads.created("a");
        // The first document commits: still the first deadline.
        assert_eq!(loads.committed("a", true), None);
        assert!(loads.stall("a", first));
        // The held request is answered while the card is up: the commit
        // leaves it stalled, so its Finished is judged for a reopen.
        assert_eq!(loads.committed("a", true), None);
        assert_eq!(loads.settle("a"), Settled::AfterStall);
        // A later navigation, after a settled load, is watched anew.
        assert!(loads.committed("a", true).is_some());
    }

    #[test]
    fn keep_waiting_gives_a_new_deadline_and_a_file_after_a_stall_stays_on_the_card() {
        let mut loads = Loads::default();
        let first = loads.created("a");
        assert!(loads.stall("a", first));
        let second = loads.rearm("a");
        assert!(!loads.stall("a", first));
        assert!(loads.stall("a", second));
        assert_eq!(loads.settle("a"), Settled::AfterStall);
        loads.restall("a");
        assert!(loads.stalled("a"));
    }

    #[test]
    fn every_commit_is_a_new_document_with_nothing_said_about_it() {
        let mut loads = Loads::default();
        loads.created("a");
        let doc = loads.doc("a");
        loads.ui_seen("a");
        loads.note(
            "a",
            "answered 502 (Bad gateway): it may be restarting".into(),
        );
        assert!(loads.ui("a") && loads.noted("a").is_some());
        loads.committed("a", true);
        assert_eq!(loads.doc("a"), doc + 1);
        assert!(!loads.ui("a"));
        assert_eq!(loads.noted("a"), None);
        // And a closed webview forgets its page, not its prewarm count.
        loads.ui_seen("a");
        assert_eq!(loads.prewarm_failed("a"), 1);
        loads.forget("a");
        assert!(!loads.ui("a"));
        assert_eq!(loads.prewarm_failed("a"), 2);
    }

    #[test]
    fn a_page_whose_ui_is_up_is_not_a_stall() {
        let mut loads = Loads::default();
        let generation = loads.created("a");
        loads.up("a", generation);
        assert!(!loads.stall("a", generation));
        let newer = loads.created("a");
        loads.up("a", generation);
        assert!(loads.stall("a", newer));
    }

    #[test]
    fn a_page_loaded_hidden_at_launch_is_retried_with_backoff() {
        let mut loads = Loads::default();
        let waits: Vec<_> = (1..=6)
            .map(|_| prewarm_backoff(loads.prewarm_failed("b")))
            .collect();
        assert_eq!(
            waits,
            vec![
                Some(Duration::from_secs(60)),
                Some(Duration::from_secs(120)),
                Some(Duration::from_secs(300)),
                Some(Duration::from_secs(900)),
                Some(Duration::from_secs(1800)),
                None,
            ]
        );
        // It came up: the next failure starts over.
        loads.prewarm_ok("b");
        assert_eq!(
            prewarm_backoff(loads.prewarm_failed("b")),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn the_platforms_words_for_a_load() {
        assert_eq!(
            page_reason(Some(502), None).unwrap(),
            "answered 502 (Bad gateway): it may be restarting"
        );
        assert_eq!(
            page_reason(Some(524), None).unwrap(),
            "answered 524 (Cloudflare: a timeout occurred): it may be restarting"
        );
        assert_eq!(
            page_reason(Some(403), None).unwrap(),
            "answered 403 (Forbidden)"
        );
        assert_eq!(
            page_reason(Some(404), None).unwrap(),
            "answered 404 (Not found)"
        );
        assert_eq!(
            page_reason(Some(599), None).unwrap(),
            "answered 599: it may be restarting"
        );
        assert_eq!(page_reason(Some(200), None), None);
        assert_eq!(page_reason(Some(302), None), None);
        assert_eq!(page_reason(None, None), None);
        assert_eq!(
            page_reason(None, Some("refused")).unwrap(),
            "did not load: refused"
        );
    }

    #[test]
    fn webview2_navigation_completed_says_only_what_failed() {
        // Cloudflare's pages: a status.
        assert!(
            webview2_reason(false, false, "x", 502)
                .unwrap()
                .contains("502 (Bad gateway)")
        );
        assert!(
            webview2_reason(true, false, "x", 524)
                .unwrap()
                .contains("524")
        );
        assert_eq!(
            webview2_reason(false, false, "x", 403).unwrap(),
            "answered 403 (Forbidden)"
        );
        // A page that loaded: nothing.
        assert_eq!(webview2_reason(true, false, "x", 200), None);
        // The network error page: no HTTP answer at all.
        assert_eq!(
            webview2_reason(
                false,
                false,
                webview2_error_words(WEBVIEW2_CANNOT_CONNECT),
                0
            )
            .unwrap(),
            "did not load: nothing is accepting connections there"
        );
        // Replaced, stopped, or turned into a download: nothing.
        for ended in [WEBVIEW2_CANCELLED, WEBVIEW2_ABORTED] {
            assert_eq!(
                webview2_reason(false, true, webview2_error_words(ended), 0),
                None
            );
        }
        assert!(webview2_reason(false, false, "x", -1).is_some());
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
        assert!(UI_GRACE + HOOK_WAIT < LOAD_WAIT);
    }
}
