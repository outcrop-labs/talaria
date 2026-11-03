//! OS notifications posted by the shell, not by the webview.
//!
//! WKWebView and WebKitGTK give a page's Notification API no permission
//! handler (it reads `default` and resolves `denied`), and there is no
//! site-settings UI inside the shell to change that. Desktop apps post through
//! the OS notification service instead — the same path every other app uses.
//! Remote content never gets the plugin surface: one command, clamped fields,
//! a rate limit per instance, and a same-origin path opened on click.
//!
//! This module is the part every platform shares: the clamps, the kind →
//! urgency/category rule, the (instance, tag) → banner map that makes a
//! re-post replace instead of stack — and lets a read elsewhere close the
//! banner (I-16) — the per-instance badge shares, the rate limits, the status
//! record, the OS idle time (I-22), and the click route. Each platform's
//! backend posts, closes, and reports clicks back by id:
//!
//!   * linux.rs — org.freedesktop.Notifications over ONE session-bus
//!     connection (zbus), with the desktop-entry hint, urgency, category,
//!     activation tokens, the launcher badge and the single-instance name;
//!   * macos.rs — UNUserNotificationCenter with a delegate of our own in a
//!     bundled run, the legacy center in an unbundled dev run;
//!   * windows.rs — toasts under the app's own AUMID when Windows will accept
//!     it, PowerShell's otherwise.
//!
//! Nothing here holds a lock across an `.await`.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Manager};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
use windows as os;

const TITLE_CHARS: usize = 180;
const BODY_CHARS: usize = 500;
const TAG_CHARS: usize = 80;
const HREF_BYTES: usize = 2048;
const KIND_CHARS: usize = 40;

/// The most (instance, tag) pairs remembered. The oldest-used goes first.
pub const REPLACE_CAP: usize = 512;

/// `desktop_notify`'s token bucket, per calling instance: a burst of 20,
/// then one every 2 s.
pub const POST_BURST: f64 = 20.0;
pub const POST_REFILL: Duration = Duration::from_secs(2);
/// `desktop_notification_test`, `desktop_notification_request`, and the
/// attention request a critical post makes: at most one per this, per caller.
pub const TEST_EVERY: Duration = Duration::from_secs(5);
pub const REQUEST_EVERY: Duration = Duration::from_secs(10);
pub const ATTENTION_EVERY: Duration = Duration::from_secs(60);
/// One instance's share of the badge, at most.
pub const BADGE_MAX: u32 = 99_999;

/// `desktop_notify_close`: the most tags one call may close.
pub const CLOSE_MAX: usize = 100;
/// How long one reading of the OS idle time answers every caller: pages and
/// watchers ask on schedules of their own (a page's heartbeat asks twice in
/// a few milliseconds), and the answer is the machine's, not theirs.
pub const IDLE_FRESH: Duration = Duration::from_millis(500);

/// The catch-up summary's tag (I-21): a click on it opens the bell.
pub const SUMMARY_TAG: &str = crate::watch::SUMMARY_TAG;

/// What the title of the shell's own test banner says.
pub const TEST_TITLE: &str = "Talaria test notification";
pub const TEST_BODY: &str = "notifications from this instance work";

/// The app the process registered with (`init` in setup), for the backends'
/// click callbacks, which arrive on threads of their own.
static APP: OnceLock<AppHandle> = OnceLock::new();

pub(crate) fn app() -> Option<&'static AppHandle> {
    APP.get()
}

/// A path the shell may open. Same-origin only: a leading `/`, no scheme, no
/// protocol-relative `//`, no controls, no `.` or `..` segment. Anything else
/// is dropped rather than eval'd. The same rule as the instance's own
/// (talaria-notify's `openable_href`, the page's `safeInAppHref`, the
/// worker's `safeHref`), so a row the instance refuses is one the shell
/// refuses too.
pub fn openable_href(raw: &str) -> Option<&str> {
    let href = raw.trim();
    if href.is_empty()
        || href.len() > HREF_BYTES
        || !href.starts_with('/')
        || href.starts_with("//")
    {
        return None;
    }
    if href.bytes().any(|b| b < 0x20 || b == 0x7f || b == b'\\') {
        return None;
    }
    if has_dot_segment(href) {
        return None;
    }
    Some(href)
}

/// Does the PATH part (before any `?` or `#`) hold a segment a URL parser
/// would resolve away? The WHATWG URL standard's single-dot (`.`, `%2e`) and
/// double-dot (`..`, `.%2e`, `%2e.`, `%2e%2e`) segments, ASCII
/// case-insensitive. `/..//evil.example` resolves to the PATH
/// `//evil.example`, which a second parse reads as another host, and an
/// instance URL with a path prefix (`https://host/talaria`) plus `/../x`
/// climbs out of the instance. A query or fragment is never path-resolved, so
/// a `?next=/../x` stays allowed.
fn has_dot_segment(href: &str) -> bool {
    let path = href.split(['?', '#']).next().unwrap_or("");
    path.split('/').any(|seg| {
        let s = seg.to_ascii_lowercase().replace("%2e", ".");
        s == "." || s == ".."
    })
}

/// The click, handed to the page (interface I-1). The page's router gets a
/// cancelable `talaria:notification-open` event carrying `{href, id}`; a page
/// that handles it (`preventDefault`) opens the route itself, with no reload.
/// A page with no listener — a Talaria older than it — gets today's
/// `location.assign`. Both values are JSON-encoded, so nothing in them can
/// break out of the script. `None` for an href that is not a same-origin
/// path.
pub fn open_js(raw_href: &str, id: &str) -> Option<String> {
    let href = openable_href(raw_href)?;
    let detail = open_detail(Some(href), id)?;
    Some(format!(
        "(()=>{{const d={detail};const e=new CustomEvent('talaria:notification-open',{{detail:d,cancelable:true}});if(window.dispatchEvent(e))location.assign(d.href)}})()"
    ))
}

/// I-1's detail, JSON-encoded: the path (or null) and the id.
fn open_detail(href: Option<&str>, id: &str) -> Option<String> {
    serde_json::to_string(&serde_json::json!({
        "href": href,
        "id": clamped_tag(id),
    }))
    .ok()
}

/// The summary's click (I-21) on a page that is up: the event, with the
/// summary's tag as its id, so the page opens the bell. No fallback: the
/// summary names no page to assign.
pub fn summary_js() -> String {
    let detail = open_detail(None, SUMMARY_TAG).unwrap_or_default();
    format!(
        "(()=>{{const d={detail};window.dispatchEvent(new CustomEvent('talaria:notification-open',{{detail:d,cancelable:true}}))}})()"
    )
}

/// The same click, for a page the click is creating (a suspended instance
/// restored by it): an init script, so it waits for the page's own listener
/// — dispatched at document start the event would reach nobody. It retries
/// every 250 ms until a listener takes it (`preventDefault`, as ui's
/// `NotificationToasts` does) or 20 s pass, and runs once per creation: the
/// nonce marks the session (`suspend::OPEN_FLAG`, which no restore carries
/// over), so a reload, or a later navigation in the same webview, is inert.
pub fn pending_open_js(href: Option<&str>, id: &str, nonce: u64) -> Option<String> {
    let href = match href {
        Some(raw) => Some(openable_href(raw)?),
        None => None,
    };
    let detail = open_detail(href, id)?;
    let flag = crate::suspend::OPEN_FLAG;
    Some(format!(
        r#"(() => {{
  const d = {detail};
  const nonce = '{nonce}';
  try {{
    if (sessionStorage.getItem('{flag}') === nonce) return;
    sessionStorage.setItem('{flag}', nonce);
  }} catch (e) {{}}
  const started = Date.now();
  const offer = () => {{
    const e = new CustomEvent('talaria:notification-open', {{ detail: d, cancelable: true }});
    if (!window.dispatchEvent(e)) return;
    if (Date.now() - started < 20000) setTimeout(offer, 250);
  }};
  if (document.readyState === 'loading') addEventListener('DOMContentLoaded', offer, {{ once: true }});
  else offer();
}})();"#
    ))
}

/// Strip controls and cap length. Notification titles and bodies are
/// user- and agent-written; the OS banner is not a place for a raw dump.
pub fn clamp_notice(s: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    for ch in s.chars() {
        if n >= max_chars {
            break;
        }
        if ch.is_control() {
            continue;
        }
        out.push(ch);
        n += 1;
    }
    out
}

pub fn clamped_title(raw: &str) -> String {
    clamp_notice(raw, TITLE_CHARS)
}

pub fn clamped_body(raw: &str) -> String {
    clamp_notice(raw, BODY_CHARS)
}

pub fn clamped_tag(raw: &str) -> String {
    clamp_notice(raw, TAG_CHARS)
}

/// A notification kind as the page or the server names it (`dm`,
/// `approval_pending`): `^[a-z0-9_-]{1,40}$`, or nothing at all. A kind
/// that does not fit is dropped, never guessed at.
pub fn clean_kind(raw: &str) -> Option<String> {
    let ok = !raw.is_empty()
        && raw.len() <= KIND_CHARS
        && raw
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    ok.then(|| raw.to_string())
}

/// How loudly a kind asks: the freedesktop urgency levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low = 0,
    Normal = 1,
    Critical = 2,
}

/// Needs-you kinds are critical: on GNOME and KDE they pass Do Not Disturb
/// and stay until dismissed (owner decision O-3), and the shell asks for
/// attention. Status chatter is low. Everything else, a DM included, normal.
pub fn urgency_of(kind: Option<&str>) -> Urgency {
    match kind {
        Some("approval_pending" | "judge_escalation" | "agent-problem" | "workchain_turn") => {
            Urgency::Critical
        }
        Some(
            "task-status" | "research" | "board_access" | "workchain_paused" | "gap_reported"
            | "mcp_server_down",
        ) => Urgency::Low,
        _ => Urgency::Normal,
    }
}

/// The freedesktop category for the kinds that are a person talking.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn category_of(kind: Option<&str>) -> Option<&'static str> {
    match kind {
        Some("dm" | "agent-reply" | "agent-outreach" | "mention" | "kb-comment") => {
            Some("im.received")
        }
        _ => None,
    }
}

/// A body for a server that parses markup (`body-markup`): `&`, `<` and `>`
/// as entities, so a DM saying `<b>` shows `<b>`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Where a click on a banner goes once it is clicked.
#[derive(Clone)]
pub enum Source {
    /// The instance page asked for it (`desktop_notify`), or the shell posted
    /// for it (a test): the click reveals that page and hands it the route.
    Page,
    /// The suspended instance's watcher posted it (watch.rs): the click marks
    /// the row read with the instance's own cookies, then restores the
    /// instance straight at the href.
    Watcher {
        row: String,
        origin: String,
        cookie: String,
    },
    /// The watcher's catch-up summary (I-21): the click restores the
    /// instance and hands its page `talaria:notification-open` with the
    /// summary's tag as the id, which opens the bell.
    Summary,
}

impl std::fmt::Debug for Source {
    // Never the cookie.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Page => f.write_str("Page"),
            Self::Watcher { row, origin, .. } => write!(f, "Watcher({row} at {origin})"),
            Self::Summary => f.write_str("Summary"),
        }
    }
}

/// One banner to post.
#[derive(Debug, Clone)]
pub struct Post {
    /// The webview it is for: `instance-<id>`, or `main` for the shell's own.
    pub label: String,
    /// What the person calls that instance: the banner says it, because one
    /// shell shows many instances.
    pub instance: String,
    pub title: String,
    pub body: Option<String>,
    /// Same (label, tag), same banner: a re-post replaces it.
    pub tag: String,
    pub href: Option<String>,
    pub kind: Option<String>,
    /// A re-alert: the same banner, shown again (a DM refreshed in place).
    pub renotify: bool,
    pub source: Source,
}

/// A banner the shell posted and can still act on.
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    pub tag: String,
    /// The id the notification server issued (Linux); 0 where the platform
    /// identifies a banner by (label, tag) alone.
    pub server_id: u32,
    pub href: Option<String>,
    pub source: Source,
    used: u64,
}

/// The (label, tag) → banner map, bounded. Instance B's tag never resolves
/// to A's banner, because the label is half the key, and the label is the
/// IPC caller's, never a value from the page.
#[derive(Debug, Default)]
pub struct Replaces {
    entries: Vec<Entry>,
    tick: u64,
}

impl Replaces {
    pub fn get(&self, label: &str, tag: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.label == label && e.tag == tag)
    }

    pub fn put(
        &mut self,
        label: &str,
        tag: &str,
        server_id: u32,
        href: Option<String>,
        source: Source,
    ) {
        self.tick += 1;
        let used = self.tick;
        // A server id belongs to one banner at a time.
        if server_id != 0 {
            self.entries
                .retain(|e| e.server_id != server_id || (e.label == label && e.tag == tag));
        }
        if let Some(e) = self
            .entries
            .iter_mut()
            .find(|e| e.label == label && e.tag == tag)
        {
            e.server_id = server_id;
            e.href = href;
            e.source = source;
            e.used = used;
            return;
        }
        self.entries.push(Entry {
            label: label.to_string(),
            tag: tag.to_string(),
            server_id,
            href,
            source,
            used,
        });
        if self.entries.len() > REPLACE_CAP
            && let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.used)
                .map(|(i, _)| i)
        {
            self.entries.swap_remove(oldest);
        }
    }

    pub fn by_server_id(&self, id: u32) -> Option<&Entry> {
        if id == 0 {
            return None;
        }
        self.entries.iter().find(|e| e.server_id == id)
    }

    /// The server retired the banner: a later post for its pair is a new one.
    pub fn closed(&mut self, id: u32) {
        if id != 0 {
            self.entries.retain(|e| e.server_id != id);
        }
    }

    pub fn forget_label(&mut self, label: &str) {
        self.entries.retain(|e| e.label != label);
    }

    /// Take `label`'s banner for `tag` out of the map, to close it. Only the
    /// caller's own pair: another label's tag is never found.
    pub fn take(&mut self, label: &str, tag: &str) -> Option<Entry> {
        let at = self
            .entries
            .iter()
            .position(|e| e.label == label && e.tag == tag)?;
        Some(self.entries.swap_remove(at))
    }

    /// Take every banner of `label`, to close them all.
    pub fn take_label(&mut self, label: &str) -> Vec<Entry> {
        let (mine, rest): (Vec<Entry>, Vec<Entry>) =
            self.entries.drain(..).partition(|e| e.label == label);
        self.entries = rest;
        mine
    }
}

/// A token bucket.
#[derive(Debug, Clone)]
pub struct Bucket {
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn full(now: Instant) -> Self {
        Self {
            tokens: POST_BURST,
            at: now,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let refilled =
            now.saturating_duration_since(self.at).as_secs_f64() / POST_REFILL.as_secs_f64();
        self.tokens = (self.tokens + refilled).min(POST_BURST);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Every remote-callable rate limit, per calling label.
#[derive(Debug, Default)]
pub struct Limits {
    posts: HashMap<String, Bucket>,
    last: HashMap<(&'static str, String), Instant>,
}

impl Limits {
    /// May `label` post one more banner now?
    pub fn post(&mut self, label: &str, now: Instant) -> bool {
        self.posts
            .entry(label.to_string())
            .or_insert_with(|| Bucket::full(now))
            .take(now)
    }

    /// At most one `what` per `every`, per label.
    pub fn every(
        &mut self,
        what: &'static str,
        label: &str,
        now: Instant,
        every: Duration,
    ) -> bool {
        let key = (what, label.to_string());
        match self.last.get(&key) {
            Some(at) if now.saturating_duration_since(*at) < every => false,
            _ => {
                self.last.insert(key, now);
                true
            }
        }
    }

    pub fn forget_label(&mut self, label: &str) {
        self.posts.remove(label);
        self.last.retain(|(_, l), _| l != label);
    }
}

/// Each instance's unread count. The OS shows the sum, so one instance can
/// never overwrite another's.
#[derive(Debug, Default)]
pub struct Badges {
    per: BTreeMap<String, u32>,
}

impl Badges {
    /// Set `label`'s share; the new total.
    pub fn set(&mut self, label: &str, count: u32) -> u32 {
        let count = count.min(BADGE_MAX);
        if count == 0 {
            self.per.remove(label);
        } else {
            self.per.insert(label.to_string(), count);
        }
        self.total()
    }

    /// Each label's share, for the tray menu.
    pub fn shares(&self) -> BTreeMap<String, u32> {
        self.per.clone()
    }

    pub fn remove(&mut self, label: &str) -> u32 {
        self.per.remove(label);
        self.total()
    }

    pub fn total(&self) -> u32 {
        self.per
            .values()
            .fold(0u32, |sum, n| sum.saturating_add(*n))
    }
}

/// What the person's OS lets the shell do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Granted,
    /// macOS: the person said no (System Settings → Notifications).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Denied,
    /// macOS: not asked yet.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    NotDetermined,
    /// Linux and macOS: no notification service to ask. Windows always
    /// reports Granted (notify/windows.rs).
    #[cfg_attr(target_os = "windows", allow(dead_code))]
    Unavailable,
}

/// The notification server, as it describes itself (Linux).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub spec: String,
}

/// What a backend knows about its own service.
#[derive(Debug, Clone)]
pub struct OsStatus {
    pub backend: String,
    pub permission: Permission,
    pub server: Option<ServerInfo>,
    pub capabilities: Option<Vec<String>>,
    /// The reason, in words, when it is not granted
    /// (`unavailable: org.freedesktop.DBus.Error.ServiceUnknown: …`).
    pub detail: Option<String>,
}

/// `desktop_notification_status`: the truth, for Settings → Desktop and the
/// launcher. Nothing in it is assumed: no service, no "On".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub backend: String,
    pub permission: Permission,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_posted_at: Option<i64>,
    /// The last post was handed to the service and had not been answered
    /// when the page was told (it is never reported as a plain success).
    pub pending: bool,
    pub close_to_background: bool,
    /// What closing the window does: `tray`, `menu-bar`, `minimize`, `none`.
    pub background: &'static str,
    /// The caller is muted in the launcher: it posts no banners, and its
    /// share of the badge still counts.
    pub muted: bool,
}

/// One caller's last post's outcome. Kept per label: instance B's Settings
/// never shows A's error or timing.
#[derive(Debug, Default, Clone)]
struct Recorded {
    last_error: Option<String>,
    last_posted_at: Option<i64>,
    pending: bool,
}

/// The shell's notification state (Tauri-managed). Lock discipline: each
/// mutex is taken alone and never across an `.await`.
#[derive(Default)]
pub struct Notifier {
    replaces: Mutex<Replaces>,
    limits: Mutex<Limits>,
    badges: Mutex<Badges>,
    recorded: Mutex<HashMap<String, Recorded>>,
    /// The badge total last handed to the OS, so an unchanged count is not
    /// re-sent on every page refetch.
    applied_badge: Mutex<Option<u32>>,
    /// The per-instance shares last shown in the tray menu.
    applied_shares: Mutex<Option<BTreeMap<String, u32>>>,
}

/// Set up the backend: the process's one bus connection (Linux), the
/// notification delegate (macOS), the AUMID (Windows). Call once, in setup.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
    app.manage(Notifier::default());
    os::init(app);
}

fn notifier(app: &AppHandle) -> tauri::State<'_, Notifier> {
    app.state::<Notifier>()
}

/// May this caller post one more banner now (`desktop_notify`'s bucket)?
pub fn allow_post(app: &AppHandle, label: &str) -> bool {
    notifier(app)
        .limits
        .lock()
        .unwrap()
        .post(label, Instant::now())
}

/// At most one `what` per `every` for this caller.
pub fn allow_every(app: &AppHandle, what: &'static str, label: &str, every: Duration) -> bool {
    notifier(app)
        .limits
        .lock()
        .unwrap()
        .every(what, label, Instant::now(), every)
}

/// Post one banner, replacing the one already up for its (label, tag).
pub async fn post(app: &AppHandle, post: Post) -> Result<(), String> {
    let previous = notifier(app)
        .replaces
        .lock()
        .unwrap()
        .get(&post.label, &post.tag)
        .map(|e| e.server_id)
        .filter(|id| *id != 0);
    if post.renotify {
        eprintln!(
            "talaria-desktop: {}: re-alerting {}{}",
            post.label,
            post.tag,
            previous.map_or(String::new(), |id| format!(" (was {id})"))
        );
    }
    let outcome = os::post(&post, previous).await;
    let state = notifier(app);
    match outcome {
        Ok(server_id) => {
            state.replaces.lock().unwrap().put(
                &post.label,
                &post.tag,
                server_id,
                post.href.clone(),
                post.source.clone(),
            );
            {
                let mut recorded = state.recorded.lock().unwrap();
                let recorded = recorded.entry(post.label.clone()).or_default();
                recorded.last_error = None;
                recorded.pending = false;
                recorded.last_posted_at = Some(crate::registry::now_ms());
            }
            if urgency_of(post.kind.as_deref()) == Urgency::Critical {
                attention(app, &post.label);
            }
            Ok(())
        }
        Err(e) => {
            let mut recorded = state.recorded.lock().unwrap();
            let recorded = recorded.entry(post.label.clone()).or_default();
            recorded.last_error = Some(e.clone());
            recorded.pending = false;
            Err(e)
        }
    }
}

/// The post was handed over and has not been answered yet: recorded, so the
/// status says so instead of the page reading it as a success.
pub fn record_pending(app: &AppHandle, label: &str) {
    notifier(app)
        .recorded
        .lock()
        .unwrap()
        .entry(label.to_string())
        .or_default()
        .pending = true;
}

/// A critical kind asks for the window's attention, at most once a minute
/// per instance. GTK's urgency hint on X11, the dock bounce on macOS;
/// Wayland has no protocol for it, and there urgency=critical is the alert.
fn attention(app: &AppHandle, label: &str) {
    if !allow_every(app, "attention", label, ATTENTION_EVERY) {
        return;
    }
    eprintln!("talaria-desktop: {label}: requesting attention for a critical notification");
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_window("main") {
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }
    });
}

/// The OS said a banner with this server id was clicked (Linux).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn clicked_server_id(id: u32, token: Option<String>) {
    let Some(app) = app() else {
        return;
    };
    let entry = notifier(app)
        .replaces
        .lock()
        .unwrap()
        .by_server_id(id)
        .cloned();
    match entry {
        Some(entry) => route(app, entry, token),
        None => eprintln!("talaria-desktop: a click on notification {id}, which is not ours"),
    }
}

/// The OS said this (label, tag)'s banner was clicked (macOS, Windows). A
/// pair the map no longer holds still opens its instance.
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn clicked_tag(label: &str, tag: &str) {
    let Some(app) = app() else {
        return;
    };
    let entry = notifier(app)
        .replaces
        .lock()
        .unwrap()
        .get(label, tag)
        .cloned()
        .unwrap_or_else(|| Entry {
            label: label.to_string(),
            tag: tag.to_string(),
            server_id: 0,
            href: None,
            source: Source::Page,
            used: 0,
        });
    route(app, entry, None);
}

/// The server retired a banner (Linux `NotificationClosed`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn closed_server_id(id: u32) {
    if let Some(app) = app() {
        notifier(app).replaces.lock().unwrap().closed(id);
    }
}

fn route(app: &AppHandle, entry: Entry, token: Option<String>) {
    eprintln!(
        "talaria-desktop: {}: notification {} clicked{}",
        entry.label,
        entry.tag,
        if token.is_some() {
            " (with an activation token)"
        } else {
            ""
        }
    );
    match entry.source {
        Source::Page => crate::commands::reveal_notice(
            app,
            &entry.label,
            entry.href.as_deref(),
            Some(&entry.tag),
            token,
        ),
        Source::Summary => {
            crate::commands::reveal_notice(app, &entry.label, None, Some(SUMMARY_TAG), token)
        }
        Source::Watcher {
            row,
            origin,
            cookie,
        } => {
            let app = app.clone();
            let label = entry.label;
            let href = entry.href;
            tauri::async_runtime::spawn(async move {
                match mark_read(&origin, &cookie, &row).await {
                    Ok(()) => eprintln!("talaria-desktop: {label}: marked {row} read"),
                    Err(e) => eprintln!("talaria-desktop: {label}: marking {row} read: {e}"),
                }
                crate::commands::reveal_notice(&app, &label, href.as_deref(), Some(&row), token);
            });
        }
    }
}

/// `PUT /api/notifications {ids:[id]}` as the instance page would, with its
/// own cookies. The session cookie is SameSite=Lax and writes need no CSRF
/// token; the Origin is the instance's own.
async fn mark_read(origin: &str, cookie: &str, id: &str) -> Result<(), String> {
    use reqwest::header::{CONTENT_TYPE, COOKIE, ORIGIN};
    let client = crate::build_client(|b| {
        b.timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
    });
    let response = client
        .put(format!("{origin}/api/notifications"))
        .header(COOKIE, cookie)
        .header(ORIGIN, origin)
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::json!({ "ids": [id] }).to_string())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("the instance answered {}", response.status()))
    }
}

/// Set `label`'s share of the badge; the OS shows the sum.
pub fn set_badge(app: &AppHandle, label: &str, count: u32) {
    let total = notifier(app).badges.lock().unwrap().set(label, count);
    apply_badge(app, total);
}

/// Each instance's share of the badge (label → count; zero shares absent).
pub fn badge_shares(app: &AppHandle) -> BTreeMap<String, u32> {
    app.try_state::<Notifier>()
        .map(|n| n.badges.lock().unwrap().shares())
        .unwrap_or_default()
}

/// Close `label`'s banners for these tags (I-16, I-22): a read elsewhere,
/// told by the page (`desktop_notify_close`) or by the watcher's stream.
/// Only the caller's own pairs: the label is half the key, and it is the
/// IPC caller's, so instance B's tags never close A's banners. The number
/// closed.
pub async fn close(app: &AppHandle, label: &str, tags: &[String]) -> u32 {
    if tags.is_empty() {
        return 0;
    }
    let entries: Vec<Entry> = {
        let state = notifier(app);
        let mut replaces = state.replaces.lock().unwrap();
        tags.iter()
            .filter_map(|tag| replaces.take(label, tag))
            .collect()
    };
    os::close(label, tags, &entries).await
}

/// Close every banner `label` has up (I-16's `all`).
pub async fn close_label(app: &AppHandle, label: &str) -> u32 {
    let entries = notifier(app).replaces.lock().unwrap().take_label(label);
    let tags: Vec<String> = entries.iter().map(|e| e.tag.clone()).collect();
    os::close(label, &tags, &entries).await
}

/// Milliseconds since the person last touched this machine, as the OS
/// reports it (I-22), or None where it cannot say: presence's "active" is
/// OS input in the last two minutes. One reading serves every caller for
/// `IDLE_FRESH` (aged by the time since), so no caller, however often it
/// asks, sends more than two bus calls a second.
pub async fn os_idle() -> Option<u64> {
    static LAST: Mutex<Option<(Instant, Option<u64>)>> = Mutex::new(None);
    let cached = *LAST.lock().unwrap();
    if let Some((at, value)) = cached
        && at.elapsed() < IDLE_FRESH
    {
        return value.map(|ms| ms.saturating_add(at.elapsed().as_millis() as u64));
    }
    let value = os::os_idle().await;
    *LAST.lock().unwrap() = Some((Instant::now(), value));
    value
}

/// An instance is gone: its badge share, its banners' map entries, its rate
/// limits and its last outcome go with it.
pub fn forget_label(app: &AppHandle, label: &str) {
    let state = notifier(app);
    let total = state.badges.lock().unwrap().remove(label);
    state.replaces.lock().unwrap().forget_label(label);
    state.limits.lock().unwrap().forget_label(label);
    state.recorded.lock().unwrap().remove(label);
    apply_badge(app, total);
}

pub fn badge_total(app: &AppHandle) -> u32 {
    notifier(app).badges.lock().unwrap().total()
}

fn apply_badge(app: &AppHandle, total: u32) {
    let state = notifier(app);
    let shares = state.badges.lock().unwrap().shares();
    let shares_changed = {
        let mut applied = state.applied_shares.lock().unwrap();
        let changed = applied.as_ref() != Some(&shares);
        *applied = Some(shares);
        changed
    };
    if shares_changed {
        // The tray menu's per-instance counts, and the launcher's list.
        crate::tray::refresh_menu(app);
        crate::commands::emit_state(app);
    }
    {
        let mut applied = state.applied_badge.lock().unwrap();
        if *applied == Some(total) {
            return;
        }
        *applied = Some(total);
    }
    eprintln!("talaria-desktop: badge total {total}");
    os::set_badge(app, total);
    crate::tray::set_unread(app, total);
}

/// The full status for the caller `label`: what the backend reports, what
/// that caller's last post did, and what closing the window does.
pub async fn status(app: &AppHandle, label: &str) -> Status {
    let os = os::probe().await;
    combine(app, os, label)
}

/// Ask the OS for permission where it has a prompt (macOS: from this click,
/// never at launch; denied: open System Settings), then report.
pub async fn request(app: &AppHandle, label: &str) -> Status {
    let os = os::request().await;
    combine(app, os, label)
}

fn combine(app: &AppHandle, os: OsStatus, label: &str) -> Status {
    let Recorded {
        last_error,
        last_posted_at,
        pending,
    } = notifier(app)
        .recorded
        .lock()
        .unwrap()
        .get(label)
        .cloned()
        .unwrap_or_default();
    Status {
        backend: os.backend,
        permission: os.permission,
        server: os.server,
        capabilities: os.capabilities,
        detail: os.detail,
        last_error,
        last_posted_at,
        pending,
        close_to_background: crate::background::close_to_background(app),
        background: crate::background::mode(app).as_str(),
        muted: crate::commands::is_muted(app, label),
    }
}

/// `--notify-selftest[=<path>]`, parsed before the Tauri builder: `Some` with
/// the report's path (`None` for stdout) when the flag is present.
pub fn selftest_arg(args: impl IntoIterator<Item = String>) -> Option<Option<std::path::PathBuf>> {
    for arg in args {
        if arg == "--notify-selftest" {
            return Some(None);
        }
        if let Some(path) = arg.strip_prefix("--notify-selftest=") {
            return Some((!path.is_empty()).then(|| std::path::PathBuf::from(path)));
        }
    }
    None
}

/// Run the backend's self-test without a window and write its JSON report:
/// `{backend, bundleId, authorization, post, background, …}`. CI runs it on
/// every platform's packaged build (desktop-package.yml). It posts one real
/// banner where the OS allows it, so it is not for a person's own desktop.
pub fn selftest(path: Option<std::path::PathBuf>) -> i32 {
    let report = os::selftest();
    let json = serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into());
    match path {
        Some(path) => match std::fs::write(&path, &json) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("talaria-desktop: writing {}: {e}", path.display());
                1
            }
        },
        None => {
            println!("{json}");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openable_href_keeps_a_same_origin_path() {
        assert_eq!(openable_href("/comms?c=1"), Some("/comms?c=1"));
        assert_eq!(openable_href("  /inbox  "), Some("/inbox"));
        assert_eq!(openable_href("/"), Some("/"));
    }

    #[test]
    fn openable_href_rejects_anything_that_is_not_a_path() {
        assert_eq!(openable_href("https://evil.example"), None);
        assert_eq!(openable_href("//evil.example"), None);
        assert_eq!(openable_href("javascript:alert(1)"), None);
        assert_eq!(openable_href("/foo\nbar"), None);
        assert_eq!(openable_href("/foo\\bar"), None);
        assert_eq!(openable_href(""), None);
        assert_eq!(openable_href("   "), None);
    }

    // The instance's table of cases, verbatim: talaria-notify's
    // `openable_href_refuses_dot_segments_in_every_spelling`,
    // ui/src/lib/notify-open.test.ts and ui/src/sw.test.ts hold the same
    // one, so the server, the page, the worker and the shell agree.
    const DOT_SEGMENT_HREFS: &[&str] = &[
        "/..//evil.example",
        "/.//evil.example",
        "/a/..//evil.example",
        "/%2e%2e//evil.example",
        "/%2E%2e//evil.example",
        "/.%2e//evil.example",
        "/%2e.//evil.example",
        "/%2e//evil.example",
        "/a/b/../../..//evil.example",
        "/boards/..",
        "/boards/.",
        "/./boards",
    ];
    const DOT_LOOKALIKES: &[&str] = &[
        "/.../x",
        "/..a//x",
        "/a.b/c",
        "/x?next=/..//evil.example",
        "/x#/..//evil.example",
        "/%2e%2ea/x",
    ];

    #[test]
    fn openable_href_refuses_dot_segments_in_every_spelling() {
        for bad in DOT_SEGMENT_HREFS {
            assert_eq!(openable_href(bad), None, "{bad}");
            // So the click never dispatches it, and a suspended instance is
            // never recreated at it (commands.rs `activate`).
            assert!(open_js(bad, "n-1").is_none(), "{bad}");
        }
        for good in DOT_LOOKALIKES {
            assert_eq!(openable_href(good), Some(*good), "{good}");
            assert!(open_js(good, "n-1").is_some(), "{good}");
        }
    }

    #[test]
    fn what_a_dot_segment_would_have_done() {
        // The hazard itself, through the shell's own URL parser: each of
        // these resolves to the PATH `//evil.example`, which a second parse
        // reads as another host…
        let origin = url::Url::parse("https://instance.example").unwrap();
        for bad in DOT_SEGMENT_HREFS
            .iter()
            .filter(|h| h.ends_with("//evil.example"))
        {
            assert_eq!(origin.join(bad).unwrap().path(), "//evil.example", "{bad}");
        }
        // …and after an instance URL with a path prefix (`activate` opens
        // `{instance.url}{href}`), `..` climbs out of the instance.
        let joined = url::Url::parse("https://host.example/talaria/../x").unwrap();
        assert_eq!(joined.path(), "/x");
    }

    #[test]
    fn open_js_dispatches_a_cancelable_event_and_falls_back_to_assign() {
        let js = open_js(r#"/comms?c=1&q="hi""#, "n-1").expect("path");
        assert!(
            js.contains(r#"const d={"href":"/comms?c=1&q=\"hi\"","id":"n-1"}"#),
            "{js}"
        );
        assert!(
            js.contains("new CustomEvent('talaria:notification-open',{detail:d,cancelable:true})")
        );
        assert!(js.contains("if(window.dispatchEvent(e))location.assign(d.href)"));
        assert!(open_js("https://evil.example", "n-1").is_none());
        // A quote or a closing tag in the id cannot leave the string either.
        let js = open_js("/x", "a\"</script>").unwrap();
        assert!(js.contains(r#""id":"a\"</script>""#));
    }

    #[test]
    fn the_summary_click_hands_the_page_its_tag_and_waits_for_a_restored_one() {
        let now = summary_js();
        assert!(
            now.contains(r#"const d={"href":null,"id":"talaria-summary"}"#),
            "{now}"
        );
        assert!(
            !now.contains("location.assign"),
            "the summary names no page"
        );
        let pending = pending_open_js(None, SUMMARY_TAG, 42).unwrap();
        assert!(pending.contains(r#"const d = {"href":null,"id":"talaria-summary"};"#));
        assert!(pending.contains("const nonce = '42';"));
        assert!(pending.contains("sessionStorage.setItem('__talaria_desktop_open', nonce)"));
        assert!(pending.contains("if (!window.dispatchEvent(e)) return;"));
        assert!(pending.contains("setTimeout(offer, 250)"));
        // A row's open carries its path; a path the rule refuses is no open.
        let row = pending_open_js(Some("/comms/channel/c1"), "n-1", 1).unwrap();
        assert!(row.contains(r#""href":"/comms/channel/c1","id":"n-1""#));
        assert!(pending_open_js(Some("//evil.example"), "n-1", 1).is_none());
    }

    #[test]
    fn closing_takes_only_the_callers_own_pairs() {
        let mut map = Replaces::default();
        map.put("instance-a", "n-1", 7, None, Source::Page);
        map.put("instance-a", "n-2", 8, None, Source::Page);
        map.put("instance-b", "n-1", 9, None, Source::Page);
        // B closing "n-1" is B's banner, never A's.
        assert_eq!(map.take("instance-b", "n-1").unwrap().server_id, 9);
        assert!(map.take("instance-b", "n-1").is_none());
        assert_eq!(map.get("instance-a", "n-1").unwrap().server_id, 7);
        // An unknown tag closes nothing.
        assert!(map.take("instance-a", "nope").is_none());
        let all = map.take_label("instance-a");
        assert_eq!(all.len(), 2);
        assert!(map.entries.is_empty());
        assert!(map.take_label("instance-a").is_empty());
    }

    #[test]
    fn clamp_notice_drops_controls_and_caps_length() {
        assert_eq!(clamp_notice("a\nb\u{0007}c", 10), "abc");
        assert_eq!(clamp_notice("abcdef", 3), "abc");
    }

    #[test]
    fn a_kind_is_kept_only_when_it_is_a_plain_slug() {
        assert_eq!(clean_kind("dm").as_deref(), Some("dm"));
        assert_eq!(
            clean_kind("approval_pending").as_deref(),
            Some("approval_pending")
        );
        assert_eq!(clean_kind("agent-reply").as_deref(), Some("agent-reply"));
        let long = "a".repeat(41);
        for junk in ["", "DM", "dm ", "d.m", "<b>", long.as_str(), "dm\n", "dé"] {
            assert_eq!(clean_kind(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn needs_you_kinds_are_critical_status_is_low_the_rest_normal() {
        for k in [
            "approval_pending",
            "judge_escalation",
            "agent-problem",
            "workchain_turn",
        ] {
            assert_eq!(urgency_of(Some(k)), Urgency::Critical, "{k}");
        }
        for k in [
            "task-status",
            "research",
            "board_access",
            "workchain_paused",
            "gap_reported",
            "mcp_server_down",
        ] {
            assert_eq!(urgency_of(Some(k)), Urgency::Low, "{k}");
        }
        for k in [Some("dm"), Some("mention"), Some("brand-new"), None] {
            assert_eq!(urgency_of(k), Urgency::Normal, "{k:?}");
        }
        assert_eq!(Urgency::Critical as u8, 2);
        assert_eq!(category_of(Some("dm")), Some("im.received"));
        assert_eq!(category_of(Some("kb-comment")), Some("im.received"));
        assert_eq!(category_of(Some("approval_pending")), None);
        assert_eq!(category_of(None), None);
    }

    #[test]
    fn markup_is_escaped() {
        assert_eq!(
            escape_markup("<b>& done</b>"),
            "&lt;b&gt;&amp; done&lt;/b&gt;"
        );
        assert_eq!(escape_markup("plain"), "plain");
    }

    #[test]
    fn a_re_post_finds_its_own_pair_and_never_another_instances() {
        let mut map = Replaces::default();
        map.put("instance-a", "n-1", 7, Some("/x".into()), Source::Page);
        assert_eq!(map.get("instance-a", "n-1").unwrap().server_id, 7);
        assert!(map.get("instance-b", "n-1").is_none());
        // B posting A's tag is its own banner.
        map.put("instance-b", "n-1", 9, None, Source::Page);
        assert_eq!(map.get("instance-a", "n-1").unwrap().server_id, 7);
        assert_eq!(map.by_server_id(9).unwrap().label, "instance-b");
        // The same pair re-posted keeps one entry.
        map.put("instance-a", "n-1", 7, Some("/y".into()), Source::Page);
        assert_eq!(map.entries.len(), 2);
        assert_eq!(
            map.get("instance-a", "n-1").unwrap().href.as_deref(),
            Some("/y")
        );
        // Closed: the pair's next post is a new banner.
        map.closed(7);
        assert!(map.get("instance-a", "n-1").is_none());
        assert!(map.by_server_id(0).is_none());
    }

    #[test]
    fn the_map_stays_bounded_and_drops_the_least_recently_used() {
        let mut map = Replaces::default();
        for i in 0..REPLACE_CAP {
            map.put(
                "instance-a",
                &format!("t{i}"),
                i as u32 + 1,
                None,
                Source::Page,
            );
        }
        // Touch the oldest, so t1 is now the least recently used.
        map.put("instance-a", "t0", 1, None, Source::Page);
        map.put("instance-a", "new", 10_000, None, Source::Page);
        assert_eq!(map.entries.len(), REPLACE_CAP);
        assert!(map.get("instance-a", "t0").is_some());
        assert!(map.get("instance-a", "t1").is_none());
        map.forget_label("instance-a");
        assert_eq!(map.entries.len(), 0);
    }

    #[test]
    fn a_server_id_reissued_moves_to_its_new_pair() {
        let mut map = Replaces::default();
        map.put("instance-a", "x", 3, None, Source::Page);
        map.put("instance-b", "y", 3, None, Source::Page);
        assert!(map.get("instance-a", "x").is_none());
        assert_eq!(map.by_server_id(3).unwrap().label, "instance-b");
    }

    #[test]
    fn twenty_posts_then_one_every_two_seconds() {
        let mut limits = Limits::default();
        let t0 = Instant::now();
        let posted = (0..50).filter(|_| limits.post("instance-a", t0)).count();
        assert_eq!(posted, 20);
        // Another instance has its own bucket.
        assert!(limits.post("instance-b", t0));
        // One token back every 2 s, and no sooner.
        assert!(!limits.post("instance-a", t0 + Duration::from_millis(1_900)));
        assert!(limits.post("instance-a", t0 + Duration::from_millis(2_000)));
        assert!(!limits.post("instance-a", t0 + Duration::from_millis(2_100)));
        assert!(limits.post("instance-a", t0 + Duration::from_millis(4_000)));
        // A long quiet refills to the burst, never past it.
        let later = t0 + Duration::from_secs(3_600);
        assert_eq!(
            (0..50).filter(|_| limits.post("instance-a", later)).count(),
            20
        );
    }

    #[test]
    fn one_test_per_five_seconds_per_caller() {
        let mut limits = Limits::default();
        let t0 = Instant::now();
        assert!(limits.every("test", "instance-a", t0, TEST_EVERY));
        assert!(!limits.every(
            "test",
            "instance-a",
            t0 + Duration::from_secs(4),
            TEST_EVERY
        ));
        assert!(limits.every("test", "instance-b", t0, TEST_EVERY));
        assert!(limits.every("request", "instance-a", t0, REQUEST_EVERY));
        assert!(limits.every(
            "test",
            "instance-a",
            t0 + Duration::from_secs(5),
            TEST_EVERY
        ));
    }

    #[test]
    fn the_badge_is_the_sum_and_no_instance_overwrites_another() {
        let mut badges = Badges::default();
        assert_eq!(badges.set("instance-a", 2), 2);
        assert_eq!(badges.set("instance-b", 3), 5);
        assert_eq!(badges.set("instance-b", 9), 11);
        assert_eq!(badges.set("instance-a", 0), 9);
        assert_eq!(
            badges.shares().into_iter().collect::<Vec<_>>(),
            vec![("instance-b".to_string(), 9)]
        );
        assert_eq!(badges.set("instance-c", u32::MAX), 9 + BADGE_MAX);
        assert_eq!(badges.remove("instance-c"), 9);
        assert_eq!(badges.remove("instance-b"), 0);
    }

    #[test]
    fn the_selftest_flag_is_found_with_or_without_a_path() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(selftest_arg(args(&["talaria-desktop"])), None);
        assert_eq!(selftest_arg(args(&["x", "--notify-selftest"])), Some(None));
        assert_eq!(
            selftest_arg(args(&["x", "--notify-selftest=/tmp/st.json"])),
            Some(Some("/tmp/st.json".into()))
        );
        assert_eq!(selftest_arg(args(&["x", "--notify-selftest="])), Some(None));
    }

    #[test]
    fn the_status_speaks_the_bridge_vocabulary() {
        let status = Status {
            backend: "fdo".into(),
            permission: Permission::NotDetermined,
            server: Some(ServerInfo {
                name: "recorder".into(),
                vendor: "talaria-test".into(),
                version: "1.0".into(),
                spec: "1.2".into(),
            }),
            capabilities: None,
            detail: None,
            last_error: None,
            last_posted_at: Some(1),
            pending: false,
            close_to_background: true,
            background: "minimize",
            muted: true,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["permission"], "not-determined");
        assert_eq!(json["server"]["spec"], "1.2");
        assert_eq!(json["closeToBackground"], true);
        assert_eq!(json["lastPostedAt"], 1);
        assert_eq!(json["muted"], true);
        assert!(json.get("capabilities").is_none());
        assert_eq!(
            serde_json::to_value(Permission::Unavailable).unwrap(),
            "unavailable"
        );
    }

    #[test]
    fn a_watcher_source_never_prints_its_cookie() {
        let s = Source::Watcher {
            row: "r1".into(),
            origin: "https://a.example".into(),
            cookie: "talaria_session=secret".into(),
        };
        assert!(!format!("{s:?}").contains("secret"));
    }
}
