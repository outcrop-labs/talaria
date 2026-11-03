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
    time::{Duration, Instant},
};

/// How long a page may take to load before the shell offers a way back.
/// Cloudflare holds a request to a restarting origin for about 100 s; a
/// Talaria page that is up finishes in a second or two.
pub const LOAD_WAIT: Duration = Duration::from_secs(25);
/// Keep waiting: how much longer, before the card comes back.
pub const KEEP_WAIT: Duration = Duration::from_secs(60);
/// How long the page may take to answer `PAGE_JS`.
pub const HOOK_WAIT: Duration = Duration::from_secs(3);
/// After a page finished, how long before it is first judged. The hook is set
/// before the app mounts, in the entry chunk, so it is there before `load`. A
/// Talaria from before the hook has only its titlebar's `get_desktop_settings`
/// call, which comes after the lazy Login or AppLayout chunk mounts and a
/// lazy `import('@tauri-apps/api/core')` — after `load`, and on a cold cache
/// or a slow link well after it. So a page that is Talaria's own shell
/// (`boot`) is looked at again, `BOOT_LOOKS` later, before anything happens.
pub const UI_GRACE: Duration = Duration::from_secs(3);
/// The further looks at a page that is Talaria's shell but has not said its
/// UI is up (or that did not answer at all), after `UI_GRACE`: at about +8 s
/// and +20 s from Finished, about +29 s at worst with each look's
/// `HOOK_WAIT`. They cannot race the watchdog: Finished settles the watch
/// before the first look, and a newer document drops them.
pub const BOOT_LOOKS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(12)];

/// Asked of a page: what is it? `ui` (the Talaria UI's hook), `boot`
/// (Talaria's own shell — `#root` and a module script, which no error page
/// has — whose UI has not said it is up yet), `html` (an HTML page without
/// either: an error page, a challenge, a gate, another app), `other` (a file
/// shown in the window). One expression, so eval answers its value.
pub const PAGE_JS: &str = "(() => { try { const d = window.__TALARIA_DESKTOP__; if (typeof d === 'object' && d !== null) return 'ui'; if (document.contentType !== 'text/html') return 'other'; return document.getElementById('root') && document.querySelector('script[type=module][src]') ? 'boot' : 'html'; } catch (e) { return 'html'; } })()";

/// What a page is, from `PAGE_JS` (or the titlebar's call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Ui,
    Boot,
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
        Some("boot") => Page::Boot,
        Some("html") => Page::Html,
        Some("other") => Page::Other,
        _ => Page::Silent,
    }
}

/// The platform's words on a document's load, and whether anything answered
/// at all (an HTTP status) or the load failed before an answer (the network).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noted {
    pub words: String,
    pub answered: bool,
}

/// What a page on the instance's own origin that finished is (rule 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The Talaria UI is up.
    Up,
    /// Talaria's own shell, still starting: look again.
    Boot,
    /// The page gave the shell's script no answer, and nothing said why: a
    /// page too busy to answer, or one where no script runs. Looked at again;
    /// never kept, since the way back cannot land on it.
    Silent,
    /// A file the person opened: not the shell's business.
    Leave,
    /// Something answered that is not Talaria — an error page, a challenge, a
    /// sign-in gate: the card, with the page kept behind it, since it may be
    /// working or need the person.
    Blocked(String),
    /// Nothing answered (the network): the card, and there is nothing to keep.
    Failed(String),
}

/// The words when the platform had none: a page answered, and it is not
/// Talaria.
pub const NOT_TALARIA: &str = "answered with a page that is not Talaria: it may be restarting, or something in front of it (a sign-in gate, a challenge) is waiting for you";
/// The words for Talaria's shell that never said its UI was up.
pub const NOT_STARTED: &str = "has not finished starting: its page may be slow to load, or it is a page the desktop app does not show";
/// The words for a page that never answered the shell's script.
pub const NO_ANSWER: &str = "showed a page the desktop app cannot reach into: it may be restarting";

pub fn verdict(ui_seen: bool, page: Page, noted: Option<&Noted>) -> Verdict {
    if ui_seen || page == Page::Ui {
        return Verdict::Up;
    }
    match noted {
        Some(n) if !n.answered => Verdict::Failed(n.words.clone()),
        // An error page the shell's script cannot run in (WebView2's own
        // page for an empty 5xx): nothing to keep, no pill can land (S10).
        Some(n) if page == Page::Silent => Verdict::Failed(n.words.clone()),
        Some(n) => Verdict::Blocked(n.words.clone()),
        None if page == Page::Boot => Verdict::Boot,
        None if page == Page::Other => Verdict::Leave,
        None if page == Page::Silent => Verdict::Silent,
        None => Verdict::Blocked(NOT_TALARIA.into()),
    }
}

/// What the shell does after judging a page that finished, the `attempt`th
/// look at it (0 is the first, `UI_GRACE` after Finished). `exempt`: the
/// person chose to see it as it is (Show the page), so it is never carded
/// and kept again; only its UI coming up, or a page that cannot be kept,
/// changes anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The UI is up: open it if its card is up (a stall, a challenge it
    /// passed), and take the way-back pill down.
    Reopen,
    /// Look again after this long.
    Look(Duration),
    /// A file after a stall: it stays on the card.
    Restall,
    /// Nothing to do.
    Nothing,
    /// The card, with these words. `keep`: the page stays alive behind it
    /// (Show the page); otherwise it is closed.
    Card { words: String, keep: bool },
}

pub fn after_finish(settled: Settled, verdict: Verdict, attempt: usize, exempt: bool) -> Action {
    let action = match verdict {
        Verdict::Up => Action::Reopen,
        Verdict::Boot => match BOOT_LOOKS.get(attempt) {
            Some(wait) => Action::Look(*wait),
            None => Action::Card {
                words: NOT_STARTED.into(),
                keep: true,
            },
        },
        Verdict::Silent => match BOOT_LOOKS.get(attempt) {
            Some(wait) => Action::Look(*wait),
            None => Action::Card {
                words: NO_ANSWER.into(),
                keep: false,
            },
        },
        Verdict::Leave if settled == Settled::AfterStall => Action::Restall,
        Verdict::Leave => Action::Nothing,
        Verdict::Blocked(words) => Action::Card { words, keep: true },
        Verdict::Failed(words) => Action::Card { words, keep: false },
    };
    match action {
        Action::Card { keep: true, .. } | Action::Restall if exempt => Action::Nothing,
        action => action,
    }
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
    noted: HashMap<String, Noted>,
    /// The current document finished loading.
    finished: HashSet<String>,
    /// Pages shown without their UI (Show the page, Keep waiting): they carry
    /// the way-back pill, and the card's words to put back if the person goes
    /// back to the launcher with the page still not Talaria.
    shown: HashMap<String, Shown>,
    /// Pages kept behind a card that are not Talaria. Only Show the page
    /// shows one as it is; any other open loads it fresh (`open_fresh`).
    kept: HashSet<String>,
    /// Failed loads in a row of a page loaded hidden at launch.
    prewarm: HashMap<String, u32>,
}

/// A page the person chose to see without its UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// Show the page: judged no more until its UI comes up. (Keep waiting
    /// still waits, and is judged.)
    pub exempt: bool,
    /// The card it came from, put back when the person goes back.
    pub words: Option<String>,
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
        self.finished.remove(id);
        self.kept.remove(id);
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
        if !own_origin || self.shown.get(id).is_some_and(|s| s.exempt) {
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
        self.kept.remove(id);
    }

    /// The page is kept behind its card: it answered, and it is not Talaria.
    pub fn keep(&mut self, id: &str) {
        self.kept.insert(id.to_string());
    }

    /// An open that is not Show the page, of a page kept behind its card and
    /// not already shown as it is: close it and load the instance fresh,
    /// never show the page that was not Talaria without its card or its way
    /// back (B2).
    pub fn open_fresh(&self, id: &str) -> bool {
        self.kept.contains(id) && !self.shown.contains_key(id)
    }

    pub fn ui(&self, id: &str) -> bool {
        self.ui.contains(id)
    }

    /// The platform's words on the current document: `answered` an HTTP
    /// status, or not (the network). macOS has none to give.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn note(&mut self, id: &str, words: String, answered: bool) {
        self.noted.insert(id.to_string(), Noted { words, answered });
    }

    pub fn noted(&self, id: &str) -> Option<&Noted> {
        self.noted.get(id)
    }

    /// Has the current document finished loading?
    pub fn finished(&self, id: &str) -> bool {
        self.finished.contains(id)
    }

    /// The person chose to see the page without its UI.
    pub fn show(&mut self, id: &str, exempt: bool, words: Option<String>) {
        self.shown.insert(id.to_string(), Shown { exempt, words });
    }

    pub fn shown(&self, id: &str) -> Option<&Shown> {
        self.shown.get(id)
    }

    /// Its UI is up, or it is gone: no pill, judged again.
    pub fn unshow(&mut self, id: &str) -> bool {
        self.shown.remove(id).is_some()
    }

    /// The page finished loading.
    pub fn settle(&mut self, id: &str) -> Settled {
        self.finished.insert(id.to_string());
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
        self.finished.remove(id);
        self.shown.remove(id);
        self.kept.remove(id);
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

/// The way back from a page shown without its UI (Show the page, Keep
/// waiting): a small "← Instances" button, bottom left (clear of a themed
/// titlebar the page may grow: macOS's controls, the wing mark), that opens
/// the launcher. Built from text nodes, styled through CSSOM, in a shadow
/// root — nothing a page CSP refuses — and evaluated by the shell, so the
/// page needs no help. Only a real click counts (`isTrusted`: a page's own
/// script cannot press it). On another origin (a sign-in provider the gate
/// sent it to) its call reaches the shell through the way-back grant, which
/// holds `show_welcome` alone (lib.rs::grant_way_back). Ctrl/Cmd+Shift+H does
/// the same (the escape hatch).
pub const PILL_JS: &str = r#"(() => {
  const ID = 'talaria-desktop-back';
  const root = document.documentElement;
  if (!root || document.getElementById(ID)) return;
  const host = document.createElement('div');
  host.id = ID;
  host.style.cssText = 'all:initial;position:fixed;bottom:12px;left:12px;z-index:2147483647;display:block;';
  const shadow = host.attachShadow({ mode: 'open' });
  const b = document.createElement('button');
  b.type = 'button';
  b.textContent = '← Instances';
  b.title = 'Back to your instances (Ctrl+Shift+H)';
  b.style.cssText = 'font-family:"IBM Plex Sans",ui-sans-serif,system-ui,"Segoe UI",Helvetica,Arial,sans-serif;font-weight:600;font-size:12px;line-height:1.4;padding:4px 10px;border-radius:6px;border:1px solid #4a4640;background:#141312;color:#e7e2db;cursor:pointer;box-shadow:0 4px 16px rgba(0,0,0,0.45);';
  b.addEventListener('click', (e) => {
    e.stopPropagation();
    if (!e.isTrusted || !window.__TAURI_INTERNALS__) return;
    window.__TAURI_INTERNALS__.invoke('show_welcome', {}).catch(() => {});
  });
  shadow.append(b);
  root.appendChild(host);
})()"#;

/// Takes the way-back pill down (the UI came up).
pub const PILL_OFF_JS: &str =
    "(() => { const h = document.getElementById('talaria-desktop-back'); if (h) h.remove(); })()";

/// The way back from an instance page (`show_welcome`) is taken only for the
/// page on screen, and at most once in `WAY_BACK_GAP` per page: a page left
/// in the background, or one that calls it in a loop, cannot keep throwing
/// the person to the launcher.
pub const WAY_BACK_GAP: Duration = Duration::from_secs(1);

pub fn may_go_back(on_screen: bool, last: Option<Instant>, now: Instant) -> bool {
    on_screen && last.is_none_or(|last| now.saturating_duration_since(last) >= WAY_BACK_GAP)
}

/// An instance page's Quit (`desktop_quit`, the instance's own origin only)
/// is taken only for the page on screen, and not in the first
/// `QUIT_GRACE` after launch: a page restored at launch cannot quit the app
/// on every start.
pub const QUIT_GRACE: Duration = Duration::from_secs(10);

pub fn may_quit(on_screen: bool, since_launch: Duration) -> bool {
    on_screen && since_launch >= QUIT_GRACE
}

/// The host the shell's earlier ways back navigated to. Nothing acts on it
/// now; a navigation there is refused (commands.rs::allowed_navigation).
pub const RETIRED_HOST: &str = "talaria-desktop.invalid";

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
/// The second value: did anything answer (an HTTP status), or not.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn webview2_reason(
    success: bool,
    ended: bool,
    error: &str,
    http: i32,
) -> Option<(String, bool)> {
    let status = u16::try_from(http).ok().filter(|c| *c > 0);
    if status.is_some_and(|c| c >= 400) {
        return page_reason(status, None).map(|w| (w, true));
    }
    if success || ended || status.is_some() {
        return None;
    }
    page_reason(None, Some(error)).map(|w| (w, false))
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

    /// Hand `noted` the URL, the words and whether anything answered, for
    /// every http(s) main-frame navigation of `core` that the platform has
    /// words for. It is called
    /// inside WebView2's event handler, so it must only record
    /// (commands::navigation_noted).
    pub fn on_navigation_completed(
        core: &ICoreWebView2,
        noted: impl Fn(String, String, bool) + 'static,
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
            if let Some((what, answered)) = judge(&args) {
                noted(url, what, answered);
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

    fn judge(args: &ICoreWebView2NavigationCompletedEventArgs) -> Option<(String, bool)> {
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

    fn status(words: &str) -> Noted {
        Noted {
            words: words.into(),
            answered: true,
        }
    }
    fn network(words: &str) -> Noted {
        Noted {
            words: words.into(),
            answered: false,
        }
    }

    // Rule 1: a page with the UI up is never interrupted.
    #[test]
    fn a_page_with_the_ui_up_is_never_interrupted() {
        // Whatever the platform said about the load (a download's 5xx, an
        // aborted navigation), the UI being up wins.
        let five = status("answered 500 (Internal server error): it may be restarting");
        let gone = network("did not load: refused");
        for noted in [None, Some(&five), Some(&gone)] {
            assert_eq!(verdict(false, Page::Ui, noted), Verdict::Up);
            // A Talaria from before the hook: its titlebar asked the shell.
            for page in [Page::Boot, Page::Html, Page::Silent, Page::Other] {
                assert_eq!(verdict(true, page, noted), Verdict::Up, "{page:?}");
            }
        }
    }

    // Rule 2: a page on the instance's own origin that is not Talaria.
    #[test]
    fn a_page_that_answered_is_kept_behind_its_card_and_no_answer_is_closed() {
        let cf524 = status("answered 524 (Cloudflare: a timeout occurred): it may be restarting");
        // Cloudflare's 524 after a stall, its 403 challenge, a 404: kept.
        assert_eq!(
            verdict(false, Page::Html, Some(&cf524)),
            Verdict::Blocked(cf524.words.clone())
        );
        assert_eq!(
            verdict(false, Page::Html, None),
            Verdict::Blocked(NOT_TALARIA.into())
        );
        // No answer to the shell's script, and nothing said: looked at
        // again, never kept (S10).
        assert_eq!(verdict(false, Page::Silent, None), Verdict::Silent);
        // WebView2's own page for an empty 502: it answered, but no script
        // runs there, so no way back can land on it: closed (S10).
        assert_eq!(
            verdict(
                false,
                Page::Silent,
                Some(&status("answered 502 (Bad gateway): it may be restarting"))
            ),
            Verdict::Failed("answered 502 (Bad gateway): it may be restarting".into())
        );
        // A failed answer that is not HTML (a 502 in plain text): kept too.
        assert!(matches!(
            verdict(
                false,
                Page::Other,
                Some(&status("answered 502 (Bad gateway)"))
            ),
            Verdict::Blocked(_)
        ));
        // Nothing answered (refused, the network error page): closed.
        assert_eq!(
            verdict(false, Page::Silent, Some(&network("did not load: refused"))),
            Verdict::Failed("did not load: refused".into())
        );
        // A file the person opened, that nothing said failed: left alone.
        assert_eq!(verdict(false, Page::Other, None), Verdict::Leave);
        // Talaria's own shell, still starting: looked at again.
        assert_eq!(verdict(false, Page::Boot, None), Verdict::Boot);
    }

    // The decision after a judgement, the glue round 1's bug lived in.
    #[test]
    fn after_finish_opens_only_a_page_whose_ui_is_up() {
        let cf524 = "answered 524 (Cloudflare: a timeout occurred): it may be restarting";
        // B1: held, the stall card, then Cloudflare's 524: the card, kept,
        // never a reopen.
        assert_eq!(
            after_finish(
                Settled::AfterStall,
                verdict(false, Page::Html, Some(&status(cf524))),
                0,
                false
            ),
            Action::Card {
                words: cf524.into(),
                keep: true
            }
        );
        // macOS, where nothing is said: still the card, kept.
        assert_eq!(
            after_finish(
                Settled::AfterStall,
                verdict(false, Page::Html, None),
                0,
                false
            ),
            Action::Card {
                words: NOT_TALARIA.into(),
                keep: true
            }
        );
        // A slow start of an older UI: looked at again at +5 s and +12 s,
        // and only then the card (kept: it may still come up).
        let boot = || verdict(false, Page::Boot, None);
        assert_eq!(
            after_finish(Settled::InTime, boot(), 0, false),
            Action::Look(BOOT_LOOKS[0])
        );
        assert_eq!(
            after_finish(Settled::InTime, boot(), 1, false),
            Action::Look(BOOT_LOOKS[1])
        );
        assert_eq!(
            after_finish(Settled::InTime, boot(), 2, false),
            Action::Card {
                words: NOT_STARTED.into(),
                keep: true
            }
        );
        // The UI arriving late, on any look: open it (from its card if one
        // stands).
        for attempt in 0..3 {
            assert_eq!(
                after_finish(
                    Settled::InTime,
                    verdict(true, Page::Boot, None),
                    attempt,
                    false
                ),
                Action::Reopen
            );
            assert_eq!(
                after_finish(
                    Settled::AfterStall,
                    verdict(false, Page::Ui, None),
                    attempt,
                    false
                ),
                Action::Reopen
            );
        }
        // A Cloudflare challenge (a 403 that solves itself or wants a click):
        // the card with the page kept, so Show the page can reach it.
        assert_eq!(
            after_finish(
                Settled::InTime,
                verdict(false, Page::Html, Some(&status("answered 403 (Forbidden)"))),
                0,
                false
            ),
            Action::Card {
                words: "answered 403 (Forbidden)".into(),
                keep: true
            }
        );
        // A refused load: the card, the page closed.
        assert_eq!(
            after_finish(
                Settled::InTime,
                verdict(false, Page::Silent, Some(&network("did not load: refused"))),
                0,
                false
            ),
            Action::Card {
                words: "did not load: refused".into(),
                keep: false
            }
        );
        // A file: nothing, or back on the card after a stall.
        assert_eq!(
            after_finish(Settled::InTime, Verdict::Leave, 0, false),
            Action::Nothing
        );
        assert_eq!(
            after_finish(Settled::AfterStall, Verdict::Leave, 0, false),
            Action::Restall
        );
        // A page that never answers the shell's script: looked at again,
        // then closed with its card, never kept (S10).
        let silent = || verdict(false, Page::Silent, None);
        assert_eq!(
            after_finish(Settled::InTime, silent(), 0, false),
            Action::Look(BOOT_LOOKS[0])
        );
        assert_eq!(
            after_finish(Settled::InTime, silent(), 2, false),
            Action::Card {
                words: NO_ANSWER.into(),
                keep: false
            }
        );
        // Each look's eval can take HOOK_WAIT: the worst case.
        let worst: Duration = UI_GRACE
            + BOOT_LOOKS.iter().sum::<Duration>()
            + HOOK_WAIT * (BOOT_LOOKS.len() as u32 + 1);
        assert_eq!(worst, Duration::from_secs(29));
    }

    // A page the person chose to see as it is (Show the page).
    #[test]
    fn a_page_shown_as_it_is_is_only_opened_or_closed() {
        let blocked = || verdict(false, Page::Html, Some(&status("answered 403 (Forbidden)")));
        // Not carded and kept again: it may be the challenge they are solving.
        assert_eq!(
            after_finish(Settled::Nothing, blocked(), 0, true),
            Action::Nothing
        );
        assert_eq!(
            after_finish(Settled::Nothing, verdict(false, Page::Boot, None), 2, true),
            Action::Nothing
        );
        // Its UI came up: the way back comes down.
        assert_eq!(
            after_finish(Settled::Nothing, verdict(true, Page::Html, None), 0, true),
            Action::Reopen
        );
        // A later document the way back cannot land on, or that never
        // answered at all: back to the card, closed (S10).
        assert_eq!(
            after_finish(
                Settled::Nothing,
                verdict(
                    false,
                    Page::Silent,
                    Some(&status("answered 502 (Bad gateway)"))
                ),
                0,
                true
            ),
            Action::Card {
                words: "answered 502 (Bad gateway)".into(),
                keep: false
            }
        );
        assert_eq!(
            after_finish(
                Settled::Nothing,
                verdict(false, Page::Silent, Some(&network("did not load: offline"))),
                0,
                true
            ),
            Action::Card {
                words: "did not load: offline".into(),
                keep: false
            }
        );
        assert_eq!(
            after_finish(Settled::Nothing, Verdict::Silent, 0, true),
            Action::Look(BOOT_LOOKS[0])
        );
        assert_eq!(
            after_finish(Settled::Nothing, Verdict::Silent, 2, true),
            Action::Card {
                words: NO_ANSWER.into(),
                keep: false
            }
        );
    }

    // B2: a kept page is shown as it is only by Show the page.
    #[test]
    fn a_kept_page_opened_any_other_way_is_loaded_fresh() {
        let mut loads = Loads::default();
        loads.created("a");
        assert!(!loads.open_fresh("a"));
        loads.settle("a");
        loads.keep("a");
        // Its row, the tray, a notification, a switcher: fresh.
        assert!(loads.open_fresh("a"));
        // Show the page: shown as it is, with its way back.
        loads.show("a", true, Some("Fake A answered 524.".into()));
        assert!(!loads.open_fresh("a"));
        // Its next document is judged on its own.
        let mut loads = Loads::default();
        loads.created("b");
        loads.keep("b");
        loads.committed("b", true);
        assert!(!loads.open_fresh("b"));
        // Its UI came up behind the card: it is Talaria now.
        loads.keep("b");
        loads.ui_seen("b");
        assert!(!loads.open_fresh("b"));
        // Closed.
        loads.keep("b");
        loads.forget("b");
        assert!(!loads.open_fresh("b"));
    }

    // B3: who may take the way back, and Quit.
    #[test]
    fn only_the_page_on_screen_goes_back_once_a_second_and_quits_after_launch() {
        let t0 = Instant::now();
        // The page on screen, a real press: back.
        assert!(may_go_back(true, None, t0));
        // A page off screen (left in the background, loaded at launch): never.
        assert!(!may_go_back(false, None, t0));
        assert!(!may_go_back(false, Some(t0), t0 + Duration::from_secs(60)));
        // A burst from the page on screen: one in each WAY_BACK_GAP.
        assert!(!may_go_back(
            true,
            Some(t0),
            t0 + Duration::from_millis(100)
        ));
        assert!(!may_go_back(
            true,
            Some(t0),
            t0 + Duration::from_millis(999)
        ));
        assert!(may_go_back(true, Some(t0), t0 + WAY_BACK_GAP));
        // Quit: the page on screen, and never in the first seconds of a launch
        // (a page restored at launch cannot quit every start).
        assert!(!may_quit(true, Duration::from_secs(2)));
        assert!(!may_quit(false, Duration::from_secs(600)));
        assert!(may_quit(true, QUIT_GRACE));
    }

    #[test]
    fn a_page_shown_as_it_is_is_not_watched_and_carries_its_card() {
        let mut loads = Loads::default();
        loads.created("a");
        loads.show("a", true, Some("Fake A answered 403 (Forbidden).".into()));
        // Its next document (a challenge that reloads) is not watched.
        assert_eq!(loads.committed("a", true), None);
        assert_eq!(
            loads.shown("a").and_then(|s| s.words.as_deref()),
            Some("Fake A answered 403 (Forbidden).")
        );
        // Keep waiting shows without exempting: still watched.
        let mut loads = Loads::default();
        loads.created("b");
        assert_eq!(loads.settle("b"), Settled::InTime);
        loads.show("b", false, None);
        assert!(loads.committed("b", true).is_some());
        // The UI came up: no more pill.
        assert!(loads.unshow("b"));
        assert!(!loads.unshow("b"));
        // A finished page knows it, until its next document.
        let mut loads = Loads::default();
        loads.created("c");
        assert!(!loads.finished("c"));
        loads.settle("c");
        assert!(loads.finished("c"));
        loads.committed("c", true);
        assert!(!loads.finished("c"));
    }

    #[test]
    fn the_way_back_is_text_and_cssom_only() {
        assert!(PILL_JS.contains("textContent = '← Instances'"));
        // Bottom left: clear of a themed titlebar the page may grow.
        assert!(PILL_JS.contains("bottom:12px;left:12px"));
        assert!(PILL_JS.contains("invoke('show_welcome'"));
        // Only a real click: a page's script cannot press it. And no
        // navigation the shell would act on (B3).
        assert!(PILL_JS.contains("if (!e.isTrusted || !window.__TAURI_INTERNALS__) return;"));
        assert!(!PILL_JS.contains(RETIRED_HOST));
        assert!(!PILL_JS.contains("location"));
        assert!(!PILL_JS.contains("innerHTML"));
        assert!(!PILL_JS.contains("<style"));
        assert!(PILL_JS.contains("attachShadow"));
        assert!(PILL_OFF_JS.contains("talaria-desktop-back"));
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
        assert_eq!(page_kind(Some("\"boot\"")), Page::Boot);
        assert!(PAGE_JS.contains("getElementById('root')"));
        assert!(PAGE_JS.contains("script[type=module][src]"));
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
            true,
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
                .0
                .contains("502 (Bad gateway)")
        );
        assert!(
            webview2_reason(true, false, "x", 524)
                .unwrap()
                .0
                .contains("524")
        );
        assert_eq!(
            webview2_reason(false, false, "x", 403).unwrap(),
            ("answered 403 (Forbidden)".to_string(), true)
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
            (
                "did not load: nothing is accepting connections there".to_string(),
                false
            )
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
