//! Suspending inactive instances, and restoring the exact page.
//!
//! Every opened instance keeps a webview, and on WebKitGTK each webview is its
//! own web process: about 110 MB for a trivial page, several hundred for the
//! real UI. An instance whose page has not been on screen for the timeout
//! (`settings.suspendAfterMinutes`, 10 by default, 0 for Off) has its webview
//! closed. Showing it again by any route creates the webview at the exact page
//! it was on, at once, with no probe.
//!
//! "Not on screen" means another instance or the launcher has been showing.
//! The instance on screen counts only while the window itself is minimized or
//! hidden, never while it is merely unfocused: a page on a second monitor (an
//! agent run streaming) stays.
//!
//! The page decides whether it can go. The shell asks it (`report_js`), and a
//! page that does not answer, answers with no `__TALARIA_DESKTOP__` hook (a
//! Talaria older than the hook), or says it holds typed input a reload would
//! lose, is kept until it is shown again. Only a page that says it can go is
//! closed.
//!
//! This module is the pure part: the activity clock, the URL rule and the
//! scripts. commands.rs wires it to the window.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use url::Url;

/// How often the shell looks at the clock. A timeout lands within one tick.
pub const TICK: Duration = Duration::from_secs(5);

/// How long the shell waits for a page's report before asking again.
pub const REPORT_WAIT: Duration = Duration::from_secs(30);

/// Why a page asked to stay. Logged, and shown to nobody else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// No `__TALARIA_DESKTOP__` hook: an instance whose Talaria predates it.
    /// Its notifications and typed input are its own page's job, so it stays.
    Unsupported,
    /// Typed input a reload would lose.
    Input,
    /// Its page posts notifications, and the shell could not read them the
    /// way the page does (a proxy or certificate only the webview knows, or
    /// the instance did not answer): suspending it would silence them.
    Notify,
}

#[derive(Debug, Clone, Copy)]
struct Asked {
    nonce: u64,
    at: Instant,
}

/// The activity clock: which instance is on screen, since when each other one
/// has been off it, and since when the window has been away.
#[derive(Debug, Default)]
pub struct Activity {
    /// The instance the window shows, or None for the launcher.
    shown: Option<String>,
    hidden_at: HashMap<String, Instant>,
    /// Since when the window has been minimized or hidden.
    away_since: Option<Instant>,
    kept: HashMap<String, Kept>,
    asked: HashMap<String, Asked>,
    nonce: u64,
}

impl Activity {
    /// The window now shows `shown` (None: the launcher). The instance it
    /// showed before starts its clock; the one it shows now is on screen,
    /// which also ends whatever kept it.
    pub fn show(&mut self, shown: Option<&str>, now: Instant) {
        if self.shown.as_deref() == shown {
            return;
        }
        if let Some(previous) = self.shown.take() {
            self.hidden_at.insert(previous, now);
        }
        if let Some(id) = shown {
            self.hidden_at.remove(id);
            self.kept.remove(id);
            self.asked.remove(id);
            self.shown = Some(id.to_string());
        }
    }

    /// What the window is doing: `away` is minimized or hidden. Returns true
    /// when it has just come back, which is a show of the instance on screen.
    pub fn window(&mut self, away: bool, now: Instant) -> bool {
        match (away, self.away_since) {
            (true, None) => {
                self.away_since = Some(now);
                false
            }
            (false, Some(_)) => {
                self.away_since = None;
                if let Some(id) = &self.shown {
                    self.kept.remove(id);
                    self.asked.remove(id);
                }
                true
            }
            _ => false,
        }
    }

    /// The instance on screen, when the window shows one.
    pub fn shown(&self) -> Option<&str> {
        self.shown.as_deref()
    }

    /// An instance whose webview is gone for another reason (removed, failed).
    pub fn forget(&mut self, id: &str) {
        self.hidden_at.remove(id);
        self.kept.remove(id);
        self.asked.remove(id);
        if self.shown.as_deref() == Some(id) {
            self.shown = None;
        }
    }

    /// Since when `id`'s page has been off screen, if it is.
    pub fn inactive_since(&self, id: &str) -> Option<Instant> {
        if self.shown.as_deref() == Some(id) {
            self.away_since
        } else {
            self.hidden_at.get(id).copied()
        }
    }

    /// True when `id` has been off screen for at least `timeout`.
    pub fn is_due(&self, id: &str, now: Instant, timeout: Duration) -> bool {
        self.inactive_since(id)
            .is_some_and(|since| now.saturating_duration_since(since) >= timeout)
    }

    /// The live instances to ask now: off screen for the timeout, not kept,
    /// and not already asked within `REPORT_WAIT`. A live webview the clock
    /// has never seen hidden starts its clock here.
    pub fn due(&mut self, live: &[String], now: Instant, timeout: Duration) -> Vec<String> {
        let mut due = Vec::new();
        for id in live {
            if self.shown.as_deref() != Some(id.as_str()) && !self.hidden_at.contains_key(id) {
                self.hidden_at.insert(id.clone(), now);
            }
            if self.kept.contains_key(id) || !self.is_due(id, now, timeout) {
                continue;
            }
            if self
                .asked
                .get(id)
                .is_some_and(|a| now.saturating_duration_since(a.at) < REPORT_WAIT)
            {
                continue;
            }
            due.push(id.clone());
        }
        due
    }

    /// Record that `id`'s page is being asked; the nonce must come back.
    pub fn ask(&mut self, id: &str, now: Instant) -> u64 {
        self.nonce += 1;
        self.asked.insert(
            id.to_string(),
            Asked {
                nonce: self.nonce,
                at: now,
            },
        );
        self.nonce
    }

    /// A report arrived. True when it answers the question asked of `id`.
    pub fn answered(&mut self, id: &str, nonce: u64) -> bool {
        match self.asked.get(id) {
            Some(asked) if asked.nonce == nonce => {
                self.asked.remove(id);
                true
            }
            _ => false,
        }
    }

    /// The page asked to stay: leave it alone until it is shown again.
    pub fn keep(&mut self, id: &str, why: Kept) {
        self.kept.insert(id.to_string(), why);
    }

    #[cfg(test)]
    pub fn kept(&self, id: &str) -> Option<Kept> {
        self.kept.get(id).copied()
    }
}

/// What the instance page says when asked (`report_js`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub nonce: u64,
    /// `location.href`.
    pub url: String,
    #[serde(default)]
    pub scroll: Vec<Scroll>,
    /// The page's `sessionStorage`, as `[key, value]` pairs. A reload keeps
    /// it; a recreated webview starts a new session, so the restore seeds it
    /// back (ui keeps "where you were" in each view there).
    #[serde(default)]
    pub session: Vec<(String, String)>,
    /// The page's own answer, from ui's `__TALARIA_DESKTOP__` hook. None: the
    /// page has no hook.
    #[serde(default)]
    pub page: Option<PageState>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageState {
    #[serde(default)]
    pub version: u32,
    /// Typed input a reload would lose.
    #[serde(default)]
    pub holds_input: bool,
    /// The page would post an OS notification for an arrival now (ui's
    /// `browserNotifyEnabled`): the shell keeps delivering them while it is
    /// suspended.
    #[serde(default)]
    pub notify: bool,
}

/// One scrolled box: `path` is child indexes from `<html>` down (empty for the
/// document itself).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scroll {
    #[serde(default)]
    pub path: Vec<u32>,
    pub x: f64,
    pub y: f64,
}

/// The most scrolled boxes one page keeps, and the deepest path.
const MAX_SCROLLS: usize = 16;
const MAX_PATH: usize = 64;

/// The most `sessionStorage` a restore carries: entries, and bytes in all. A
/// page holding more starts its new session empty, as a new tab would.
const MAX_SESSION_ENTRIES: usize = 256;
const MAX_SESSION_BYTES: usize = 256 * 1024;

/// The key the restore script marks a seeded session with.
const RESTORE_FLAG: &str = "__talaria_desktop_restore";

/// The session entries worth carrying, or none if they are too many.
pub fn clean_session(entries: &[(String, String)]) -> Vec<(String, String)> {
    let entries: Vec<(String, String)> = entries
        .iter()
        .filter(|(k, _)| !k.is_empty() && k != RESTORE_FLAG)
        .cloned()
        .collect();
    let bytes: usize = entries.iter().map(|(k, v)| k.len() + v.len()).sum();
    if entries.len() > MAX_SESSION_ENTRIES || bytes > MAX_SESSION_BYTES {
        return Vec::new();
    }
    entries
}

/// The scroll positions worth restoring: finite, positive, bounded.
pub fn clean_scrolls(scrolls: &[Scroll]) -> Vec<Scroll> {
    scrolls
        .iter()
        .filter(|s| s.x.is_finite() && s.y.is_finite() && (s.x > 0.0 || s.y > 0.0))
        .filter(|s| s.path.len() <= MAX_PATH)
        .take(MAX_SCROLLS)
        .map(|s| Scroll {
            path: s.path.clone(),
            x: s.x.max(0.0).round(),
            y: s.y.max(0.0).round(),
        })
        .collect()
}

/// A page the person was on that a restore may open: same origin as the
/// instance, and nothing to do with signing in. Anything else, `None`, means
/// the instance root.
///
/// Never restored: a sign-in or account page (`/login`, `/claim`, `/join`, an
/// `/auth` or `/oauth` path, any path with `callback` in it, any `/api/` URL),
/// or a URL that carries a one-time value in its query or its fragment (a
/// `code`, `state`, `token`, `ticket`, `nonce`, `otp`, `invite` and their
/// kin). Reopening one replays a code that has already been spent, or lands on
/// a sign-in page the person has finished with.
pub fn restorable_url(origin: &str, raw: &str) -> Option<String> {
    let base = Url::parse(origin).ok()?;
    let url = Url::parse(raw).ok()?;
    if url.origin() != base.origin() || !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let segments: Vec<String> = url
        .path_segments()
        .map(|s| s.map(|p| p.to_ascii_lowercase()).collect())
        .unwrap_or_default();
    const AUTH_ROOTS: &[&str] = &[
        "login", "logout", "signin", "signout", "signup", "claim", "join", "auth", "oauth",
        "oauth2", "sso", "saml", "api", "callback",
    ];
    if segments
        .first()
        .is_some_and(|first| AUTH_ROOTS.contains(&first.as_str()))
        || segments.iter().any(|s| s.contains("callback"))
    {
        return None;
    }
    const ONE_TIME: &[&str] = &[
        "code",
        "state",
        "token",
        "access_token",
        "id_token",
        "refresh_token",
        "ticket",
        "nonce",
        "otp",
        "invite",
        "magic",
        "secret",
        "password",
        "session_state",
        "sid",
        "auth",
        "key",
    ];
    let one_time = |pairs: url::form_urlencoded::Parse<'_>| {
        pairs.into_iter().any(|(k, _)| {
            let k = k.to_ascii_lowercase();
            ONE_TIME.contains(&k.as_str()) || k.ends_with("_token") || k.ends_with("token")
        })
    };
    if one_time(url.query_pairs()) {
        return None;
    }
    if let Some(fragment) = url.fragment()
        && fragment.contains('=')
        && one_time(url::form_urlencoded::parse(
            fragment.trim_start_matches(['/', '?', '!']).as_bytes(),
        ))
    {
        return None;
    }
    // A bare root is the default anyway: nothing to remember.
    if url.path() == "/" && url.query().is_none() && url.fragment().is_none() {
        return None;
    }
    Some(url.to_string())
}

/// Asks the page whether it can be suspended. The answer comes back through
/// `desktop_suspend_report`, which only that instance's webview may call.
///
/// Collected here, not in ui, so it works the same for any Talaria version:
/// the URL and the scrolled boxes. ui's hook adds what only the page knows.
pub fn report_js(nonce: u64) -> String {
    format!(
        r#"(() => {{
  try {{
    const nonce = {nonce};
    const root = document.scrollingElement || document.documentElement;
    const path = (el) => {{
      const out = [];
      while (el && el !== document.documentElement && el.parentElement) {{
        out.unshift(Array.prototype.indexOf.call(el.parentElement.children, el));
        el = el.parentElement;
      }}
      return out;
    }};
    const scroll = [];
    if (root && (root.scrollTop > 0 || root.scrollLeft > 0)) scroll.push({{ path: [], x: root.scrollLeft, y: root.scrollTop }});
    for (const el of document.querySelectorAll('body *')) {{
      if (scroll.length >= {MAX_SCROLLS}) break;
      if (el.scrollTop > 0 || el.scrollLeft > 0) scroll.push({{ path: path(el), x: el.scrollLeft, y: el.scrollTop }});
    }}
    let session = [];
    try {{
      for (let i = 0; i < sessionStorage.length; i++) {{
        const k = sessionStorage.key(i);
        if (k !== null && k !== '{RESTORE_FLAG}') session.push([k, String(sessionStorage.getItem(k))]);
      }}
    }} catch (e) {{ session = []; }}
    let page = null;
    try {{
      const hook = window.__TALARIA_DESKTOP__;
      const state = hook && typeof hook.suspendState === 'function' ? hook.suspendState() : null;
      if (state && typeof state === 'object') {{
        page = {{ version: Number(hook.version) || 0, holdsInput: !!state.holdsInput, notify: !!state.notify }};
      }}
    }} catch (e) {{
      // A hook that throws holds what it holds: keep the page.
      page = {{ version: 1, holdsInput: true, notify: false }};
    }}
    window.__TAURI_INTERNALS__.invoke('desktop_suspend_report', {{
      report: {{ nonce, url: String(location.href), scroll, session, page }},
    }}).catch(() => {{}});
  }} catch (e) {{}}
}})();"#
    )
}

/// Put a restored page back as a reload would have kept it: its session
/// storage, then its scrolled boxes. Runs at document start in the recreated
/// webview, before the page's own scripts, and only on the page that was
/// suspended: a later navigation, a reload of it in the same session (the
/// `nonce` marks a session already seeded), or a sign-in redirect leaves it
/// inert. Session entries the new page already wrote are not overwritten.
/// The scroll keeps trying while the page renders (data arrives after the
/// document) for up to five seconds, and stops at the person's first scroll,
/// key or click, so it never fights them.
pub fn restore_js(
    url: &str,
    scrolls: &[Scroll],
    session: &[(String, String)],
    nonce: u64,
) -> Option<String> {
    if scrolls.is_empty() && session.is_empty() {
        return None;
    }
    let url = serde_json::to_string(url).ok()?;
    let scrolls = serde_json::to_string(scrolls).ok()?;
    let session = serde_json::to_string(session).ok()?;
    Some(format!(
        r#"(() => {{
  const url = {url};
  if (location.href !== url) return;
  const flag = '{RESTORE_FLAG}';
  const nonce = '{nonce}';
  try {{
    if (sessionStorage.getItem(flag) === nonce) return;
    for (const [k, v] of {session}) if (sessionStorage.getItem(k) === null) sessionStorage.setItem(k, v);
    sessionStorage.setItem(flag, nonce);
  }} catch (e) {{}}
  const pending = {scrolls};
  if (!pending.length) return;
  const started = Date.now();
  let stopped = false;
  const stop = () => {{ stopped = true; }};
  for (const type of ['wheel', 'keydown', 'pointerdown', 'touchstart']) addEventListener(type, stop, {{ capture: true, once: true, passive: true }});
  const find = (path) => {{
    let el = document.documentElement;
    if (path.length === 0) return document.scrollingElement || el;
    for (const i of path) {{ el = el && el.children[i]; }}
    return el || null;
  }};
  const step = () => {{
    if (stopped) return;
    for (let i = pending.length - 1; i >= 0; i--) {{
      const s = pending[i];
      const el = find(s.path);
      if (!el) continue;
      if (el.scrollHeight - el.clientHeight + 1 < s.y || el.scrollWidth - el.clientWidth + 1 < s.x) continue;
      el.scrollTop = s.y;
      el.scrollLeft = s.x;
      pending.splice(i, 1);
    }}
    if (pending.length && Date.now() - started < 5000) setTimeout(step, 100);
  }};
  if (document.readyState === 'loading') addEventListener('DOMContentLoaded', step, {{ once: true }});
  else step();
}})();"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_instance_off_screen_for_the_timeout_is_due_and_not_before() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.show(Some("b"), t0 + MIN);
        let live = ids(&["a", "b"]);
        assert!(
            a.due(&live, t0 + MIN * 10, MIN * 10).is_empty(),
            "9 min off"
        );
        assert_eq!(a.due(&live, t0 + MIN * 11, MIN * 10), ids(&["a"]));
        // b is on screen and the window is not away: never due.
        assert!(!a.is_due("b", t0 + MIN * 1000, MIN * 10));
    }

    #[test]
    fn the_launcher_showing_makes_every_instance_inactive() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.show(None, t0 + MIN);
        assert_eq!(a.shown(), None);
        assert!(a.is_due("a", t0 + MIN * 11, MIN * 10));
        assert!(!a.is_due("a", t0 + MIN * 10, MIN * 10));
    }

    #[test]
    fn showing_an_instance_again_resets_its_clock() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.show(Some("b"), t0 + MIN);
        a.show(Some("a"), t0 + MIN * 8);
        a.show(Some("b"), t0 + MIN * 9);
        assert!(!a.is_due("a", t0 + MIN * 18, MIN * 10));
        assert!(a.is_due("a", t0 + MIN * 19, MIN * 10));
    }

    #[test]
    fn the_shown_instance_counts_only_while_the_window_is_away() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        // Unfocused is not away: the caller never reports it. A long stretch
        // on screen is never due.
        assert!(!a.is_due("a", t0 + MIN * 60, MIN * 10));
        assert!(!a.window(true, t0 + MIN * 60), "going away is not a return");
        assert!(!a.is_due("a", t0 + MIN * 69, MIN * 10));
        assert!(a.is_due("a", t0 + MIN * 70, MIN * 10));
        // Back: on screen again, and the clock is gone.
        assert!(a.window(false, t0 + MIN * 71));
        assert!(!a.is_due("a", t0 + MIN * 200, MIN * 10));
        // A second report of the same state is not a second return.
        assert!(!a.window(false, t0 + MIN * 201));
    }

    #[test]
    fn a_hidden_instance_keeps_its_own_clock_while_the_window_is_away() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.show(Some("b"), t0 + MIN);
        a.window(true, t0 + MIN * 5);
        assert!(a.is_due("a", t0 + MIN * 11, MIN * 10), "a: off since 1 min");
        assert!(
            !a.is_due("b", t0 + MIN * 11, MIN * 10),
            "b: away since 5 min"
        );
        assert!(a.is_due("b", t0 + MIN * 15, MIN * 10));
    }

    #[test]
    fn a_page_is_asked_once_per_wait_and_only_its_answer_counts() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.show(None, t0);
        let live = ids(&["a"]);
        let now = t0 + MIN * 10;
        assert_eq!(a.due(&live, now, MIN * 10), ids(&["a"]));
        let nonce = a.ask("a", now);
        assert!(
            a.due(&live, now + Duration::from_secs(5), MIN * 10)
                .is_empty()
        );
        assert!(!a.answered("a", nonce + 1), "a stale or forged nonce");
        // No answer within the wait: asked again.
        assert_eq!(a.due(&live, now + REPORT_WAIT, MIN * 10), ids(&["a"]));
        assert!(a.answered("a", nonce));
        assert!(!a.answered("a", nonce), "an answer counts once");
    }

    #[test]
    fn a_kept_page_stays_until_it_is_shown_again() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.show(None, t0);
        let live = ids(&["a"]);
        a.keep("a", Kept::Input);
        assert_eq!(a.kept("a"), Some(Kept::Input));
        assert!(a.due(&live, t0 + MIN * 100, MIN * 10).is_empty());
        a.show(Some("a"), t0 + MIN * 100);
        assert_eq!(a.kept("a"), None);
        a.show(None, t0 + MIN * 101);
        assert_eq!(a.due(&live, t0 + MIN * 111, MIN * 10), ids(&["a"]));
    }

    #[test]
    fn a_shown_page_kept_while_minimized_is_asked_again_after_the_window_returns() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.window(true, t0);
        a.keep("a", Kept::Unsupported);
        assert!(a.due(&ids(&["a"]), t0 + MIN * 20, MIN * 10).is_empty());
        a.window(false, t0 + MIN * 21);
        a.window(true, t0 + MIN * 22);
        assert_eq!(a.due(&ids(&["a"]), t0 + MIN * 32, MIN * 10), ids(&["a"]));
    }

    #[test]
    fn a_live_webview_the_clock_never_saw_starts_counting_when_found() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        assert!(a.due(&ids(&["x"]), t0, MIN).is_empty());
        assert_eq!(a.due(&ids(&["x"]), t0 + MIN, MIN), ids(&["x"]));
    }

    #[test]
    fn forgetting_an_instance_clears_it() {
        let t0 = Instant::now();
        let mut a = Activity::default();
        a.show(Some("a"), t0);
        a.forget("a");
        assert_eq!(a.shown(), None);
        assert_eq!(a.inactive_since("a"), None);
    }

    #[test]
    fn the_exact_page_is_restorable_path_query_and_hash() {
        let o = "https://outcrop.example";
        assert_eq!(
            restorable_url(
                o,
                "https://outcrop.example/boards/b1/t9?view=list&sort=due#c-12"
            ),
            Some("https://outcrop.example/boards/b1/t9?view=list&sort=due#c-12".into())
        );
        assert_eq!(
            restorable_url("http://127.0.0.1:5303", "http://127.0.0.1:5303/channel/abc"),
            Some("http://127.0.0.1:5303/channel/abc".into())
        );
        assert_eq!(
            restorable_url(o, "https://outcrop.example/"),
            None,
            "the root"
        );
    }

    #[test]
    fn another_origin_is_never_restored() {
        let o = "https://outcrop.example";
        for raw in [
            "https://evil.example/boards",
            "http://outcrop.example/boards",
            "https://outcrop.example:8443/boards",
            "https://user:pw@outcrop.example/boards",
            "about:blank",
            "javascript:alert(1)",
            "not a url",
        ] {
            assert_eq!(restorable_url(o, raw), None, "{raw}");
        }
    }

    #[test]
    fn sign_in_and_callback_pages_are_never_restored() {
        let o = "https://outcrop.example";
        for raw in [
            "https://outcrop.example/login",
            "https://outcrop.example/login?next=/boards",
            "https://outcrop.example/LOGIN",
            "https://outcrop.example/claim?c=1",
            "https://outcrop.example/join/abc",
            "https://outcrop.example/auth/x",
            "https://outcrop.example/oauth/authorize",
            "https://outcrop.example/api/auth/google/callback?code=1&state=2",
            "https://outcrop.example/api/notifications",
            "https://outcrop.example/integrations/github/callback",
            "https://outcrop.example/x/app/oauth-callback",
            "https://outcrop.example/boards?code=abc",
            "https://outcrop.example/boards?state=abc",
            "https://outcrop.example/boards?invite=abc",
            "https://outcrop.example/settings?reset_token=abc",
            "https://outcrop.example/settings?Token=abc",
            "https://outcrop.example/home#access_token=abc&token_type=bearer",
            "https://outcrop.example/home#/?id_token=abc",
        ] {
            assert_eq!(restorable_url(o, raw), None, "{raw}");
        }
        // A hash that is a plain anchor is a page, not a credential.
        assert!(restorable_url(o, "https://outcrop.example/kb/doc#section-2").is_some());
        // A word that merely contains an auth word is still a page.
        assert!(restorable_url(o, "https://outcrop.example/channels?q=login").is_some());
    }

    #[test]
    fn scrolls_are_bounded_and_finite() {
        let mut many: Vec<Scroll> = (0..40)
            .map(|i| Scroll {
                path: vec![0, i],
                x: 0.0,
                y: f64::from(i + 1),
            })
            .collect();
        many.insert(
            0,
            Scroll {
                path: vec![],
                x: f64::NAN,
                y: 10.0,
            },
        );
        many.insert(
            0,
            Scroll {
                path: vec![1; 200],
                x: 0.0,
                y: 10.0,
            },
        );
        many.insert(
            0,
            Scroll {
                path: vec![2],
                x: 0.0,
                y: 0.0,
            },
        );
        let clean = clean_scrolls(&many);
        assert_eq!(clean.len(), MAX_SCROLLS);
        assert!(clean.iter().all(|s| s.y > 0.0 && s.path.len() == 2));
    }

    #[test]
    fn the_scripts_carry_their_values_as_json() {
        let js = report_js(42);
        assert!(js.contains("const nonce = 42;"));
        assert!(js.contains("desktop_suspend_report"));
        assert!(js.contains("sessionStorage"));
        assert_eq!(restore_js("https://a.example/x", &[], &[], 1), None);
        let js = restore_js(
            "https://a.example/x?q=\"</script>",
            &[Scroll {
                path: vec![0, 3],
                x: 0.0,
                y: 420.0,
            }],
            &[("view:kb".into(), "{\"doc\":\"d1\"}".into())],
            7,
        )
        .unwrap();
        assert!(js.contains(r#"const url = "https://a.example/x?q=\"</script>";"#));
        assert!(js.contains(r#"[{"path":[0,3],"x":0.0,"y":420.0}]"#));
        assert!(js.contains(r#"[["view:kb","{\"doc\":\"d1\"}"]]"#));
        assert!(js.contains("const nonce = '7';"));
    }

    #[test]
    fn session_storage_is_carried_only_when_it_is_small() {
        let few = vec![
            ("a".to_string(), "1".to_string()),
            (RESTORE_FLAG.to_string(), "3".to_string()),
            (String::new(), "x".to_string()),
        ];
        assert_eq!(
            clean_session(&few),
            vec![("a".to_string(), "1".to_string())]
        );
        let many: Vec<(String, String)> = (0..300).map(|i| (i.to_string(), "v".into())).collect();
        assert!(clean_session(&many).is_empty());
        let big = vec![("k".to_string(), "x".repeat(MAX_SESSION_BYTES))];
        assert!(clean_session(&big).is_empty());
    }

    #[test]
    fn a_report_parses_with_or_without_the_page_hook() {
        let r: Report = serde_json::from_str(
            r#"{"nonce":3,"url":"https://a.example/x","scroll":[{"path":[0],"x":0,"y":12}],"page":{"version":1,"holdsInput":true,"notify":true}}"#,
        )
        .unwrap();
        let page = r.page.unwrap();
        assert_eq!(
            (page.version, page.holds_input, page.notify),
            (1, true, true)
        );
        let r: Report =
            serde_json::from_str(r#"{"nonce":3,"url":"https://a.example/x","page":null}"#).unwrap();
        assert!(r.page.is_none() && r.scroll.is_empty() && r.session.is_empty());
        let r: Report = serde_json::from_str(
            r#"{"nonce":3,"url":"https://a.example/x","session":[["k","v"]],"page":null}"#,
        )
        .unwrap();
        assert_eq!(r.session, vec![("k".to_string(), "v".to_string())]);
    }
}
