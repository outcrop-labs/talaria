//! A suspended instance's notifications.
//!
//! An open page posts its own: ui's `NotificationToasts` watches
//! `GET /api/notifications` (made live by `/api/me/events`, with a 30 s poll
//! floor) and, for every row that lands while Talaria is not what the person
//! is looking at, asks the shell to post it (`desktop_notify`). A suspended
//! page posts nothing, so the shell does the same job for it, natively: the
//! same two routes, the same arrival rule, the same banner and the same click.
//!
//! The arrival rule is the one the page's tracker keeps (interface I-3): a
//! row is an arrival when it is unread, its `createdAt` is newer than the
//! high-water mark minus two minutes, and its id is unseen — or its
//! `createdAt` moved later than the one recorded, which is a re-alert (a DM
//! conversation's row refreshed in place, I-8), posted again under the same
//! tag. The watcher starts from the PAGE's record when the page hands one
//! over in its suspend report (`arrivals`), so whatever the page missed — a
//! dead stream, a sleep — is posted on the watcher's first read instead of
//! being swallowed into a fresh baseline (D2). With no record (an older
//! instance UI), the first read is the baseline, as before. More than three
//! arrivals at once become one summary banner.
//!
//! It runs only when the page said it would post (`notify` in its report: ui's
//! `browserNotifyEnabled`), with that instance's own cookies, read from its
//! webview just before it closed, and only ever to that instance's origin:
//! redirects are not followed. A stream that fails — a 502 while the instance
//! restarts, a deploy, Cloudflare dropping an idle connection — is reopened
//! with backoff (1 s doubling to 30 s, reset after a minute healthy), every
//! reopen refetches, and the 30 s poll floor keeps running through the whole
//! outage. A 401 or 403 from the instance itself (signed out, session
//! expired: its JSON, not a Cloudflare challenge page) ends it; a restore ends
//! it too, and hands back what it posted (`Handback`), so the restored page
//! never posts it again. Each read also sets the instance's share of the
//! badge from the server's `unread`.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{
    StatusCode,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap},
};
use serde::Deserialize;
use tauri::AppHandle;
use tokio::time::Instant;

/// The poll floor: the page's own `refetchInterval`.
const POLL: Duration = Duration::from_secs(30);

/// No bytes for this long and the stream is presumed dead. The server pings
/// every 25 s.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// The reconnect backoff: from this, doubling, up to the cap; a stream that
/// stayed up this long starts over at the floor.
const BACKOFF_FLOOR: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);
const HEALTHY: Duration = Duration::from_secs(60);

/// The most row ids remembered: the page's tracker keeps the same.
pub const MAX_SEEN: usize = 300;

/// A row this much older than the high-water mark is never an arrival.
pub const MARGIN_MS: i64 = 120_000;

/// More arrivals than this in one read are one summary banner.
pub const SUMMARY_OVER: usize = 3;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub read_at: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: serde_json::Value,
}

impl Row {
    fn unread(&self) -> bool {
        self.read_at.as_ref().is_none_or(serde_json::Value::is_null)
    }

    /// `createdAt` in epoch ms: the ISO string the api sends, or a number.
    pub fn created_ms(&self) -> Option<i64> {
        match &self.created_at {
            serde_json::Value::String(s) => parse_iso_ms(s),
            serde_json::Value::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    /// `createdAt` as the page records it (I-3): the server's string.
    pub fn created_text(&self) -> String {
        match &self.created_at {
            serde_json::Value::String(s) => s.clone(),
            other => self
                .created_ms()
                .map(iso_of_ms)
                .unwrap_or_else(|| other.to_string()),
        }
    }
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    notifications: Vec<Row>,
    #[serde(default)]
    unread: Option<u64>,
}

/// `YYYY-MM-DDTHH:MM:SS[.fff…][Z|±HH:MM]` in epoch ms. A string that is not
/// that is `None`, never a guess.
pub fn parse_iso_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let part = s.get(from..to)?;
        if !part.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = s.get(19..)?;
    let mut millis = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        for (i, c) in frac.bytes().take(digits.min(3)).enumerate() {
            millis += i64::from(c - b'0') * [100, 10, 1][i];
        }
        rest = frac.get(digits..)?;
    }
    let offset_secs = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let tz = rest.get(1..)?;
            let (h, m) = match tz.len() {
                5 if tz.as_bytes()[2] == b':' => (tz.get(0..2)?, tz.get(3..5)?),
                4 => (tz.get(0..2)?, tz.get(2..4)?),
                _ => return None,
            };
            let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
            sign * (h * 3600 + m * 60)
        }
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs;
    Some(secs * 1000 + millis)
}

/// Epoch ms as the api writes it: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn iso_of_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 {
        yoe + era * 400 + 1
    } else {
        yoe + era * 400
    };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// The page's arrival record for one user (I-3,
/// `localStorage['talaria.notify.arrivals.v1'][userId]`), as the suspend
/// report carries it.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct Handoff {
    pub user: String,
    #[serde(default)]
    pub hw: Option<String>,
    #[serde(default)]
    pub seen: HashMap<String, String>,
    #[serde(default)]
    pub at: Option<f64>,
}

impl Handoff {
    /// The record as the suspend report carried it, read leniently: entries
    /// that are not strings are skipped, and a record this build cannot read
    /// at all is none.
    pub fn from_value(value: &serde_json::Value) -> Option<Self> {
        let user = value.get("user")?.as_str()?.to_string();
        let hw = value
            .get("hw")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let seen = value
            .get("seen")
            .and_then(serde_json::Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let at = value.get("at").and_then(serde_json::Value::as_f64);
        Self { user, hw, seen, at }.clean()
    }

    /// A record worth starting from: a user id that is an id, and at most
    /// the bounded set the page keeps (the newest, if it sent more).
    pub fn clean(mut self) -> Option<Self> {
        if self.user.is_empty()
            || self.user.len() > 100
            || self.user.bytes().any(|b| b.is_ascii_control())
        {
            return None;
        }
        self.seen.retain(|k, _| !k.is_empty() && k.len() <= 100);
        if self.seen.len() > MAX_SEEN {
            let mut by_age: Vec<(String, String)> = self.seen.drain().collect();
            by_age.sort_by_key(|(_, at)| std::cmp::Reverse(parse_iso_ms(at).unwrap_or(i64::MIN)));
            by_age.truncate(MAX_SEEN);
            self.seen = by_age.into_iter().collect();
        }
        Some(self)
    }
}

/// One row to surface, and whether it is a re-alert of one already shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrival {
    pub row: Row,
    pub realert: bool,
}

/// The arrival tracker (I-3's rule; see the module docs).
#[derive(Debug, Default, Clone)]
pub struct Arrivals {
    hw: Option<i64>,
    seen: HashMap<String, i64>,
    primed: bool,
}

impl Arrivals {
    /// Start from the page's own record: the first read posts what it missed.
    pub fn from_handoff(handoff: &Handoff) -> Self {
        let seen = handoff
            .seen
            .iter()
            .filter_map(|(id, at)| parse_iso_ms(at).map(|ms| (id.clone(), ms)))
            .collect();
        Self {
            hw: handoff.hw.as_deref().and_then(parse_iso_ms),
            seen,
            primed: true,
        }
    }

    /// Rows to surface now, oldest first. `rows` is newest first, as
    /// `/api/notifications` returns them. The first read of an unprimed
    /// tracker is the baseline, empty or not.
    pub fn arrive(&mut self, rows: &[Row]) -> Vec<Arrival> {
        if !self.primed {
            self.primed = true;
            for row in rows {
                if let Some(at) = row.created_ms() {
                    self.seen.insert(row.id.clone(), at);
                    self.hw = Some(self.hw.map_or(at, |hw| hw.max(at)));
                }
            }
            self.trim();
            return Vec::new();
        }
        let floor = self.hw.map_or(i64::MIN, |hw| hw - MARGIN_MS);
        let mut fresh = Vec::new();
        for row in rows {
            let Some(at) = row.created_ms() else {
                continue;
            };
            if !row.unread() || at <= floor {
                continue;
            }
            let realert = match self.seen.get(&row.id) {
                None => false,
                Some(seen) if at > *seen => true,
                Some(_) => continue,
            };
            fresh.push(Arrival {
                row: row.clone(),
                realert,
            });
        }
        for arrival in &fresh {
            if let Some(at) = arrival.row.created_ms() {
                self.seen.insert(arrival.row.id.clone(), at);
                self.hw = Some(self.hw.map_or(at, |hw| hw.max(at)));
            }
        }
        self.trim();
        fresh.reverse();
        fresh
    }

    fn trim(&mut self) {
        if self.seen.len() <= MAX_SEEN {
            return;
        }
        let mut by_age: Vec<(String, i64)> = self.seen.drain().collect();
        by_age.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        by_age.truncate(MAX_SEEN);
        self.seen = by_age.into_iter().collect();
    }
}

/// What the watcher posted, for the restore to merge into the page's record
/// (I-3) so the restored page never posts it again.
#[derive(Debug, Default)]
pub struct Handback {
    /// The record's user, from the page's handoff. None: the page keeps no
    /// record (an older UI), and there is nothing to merge into.
    pub user: Option<String>,
    /// Row id → `createdAt` as surfaced.
    pub posted: Vec<(String, String)>,
}

pub type Shared = Arc<Mutex<Handback>>;

/// An SSE body, fed as it arrives: complete events come out, a partial one
/// waits for the rest. Bytes are buffered and only a complete event is
/// decoded, so a character or a CRLF split across two chunks survives; every
/// line ending (CRLF, LF, a lone CR) is a newline, as the spec says.
#[derive(Debug, Default)]
pub struct Frames {
    buf: Vec<u8>,
    /// The last byte seen was a CR: an LF next completes it.
    cr: bool,
}

impl Frames {
    /// The `data` of every complete event in what has arrived so far.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        for &byte in chunk {
            if std::mem::take(&mut self.cr) {
                self.buf.push(b'\n');
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' {
                self.cr = true;
            } else {
                self.buf.push(byte);
            }
        }
        let mut out = Vec::new();
        while let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") {
            let event: Vec<u8> = self.buf.drain(..end + 2).collect();
            let event = String::from_utf8_lossy(&event);
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if !data.is_empty() {
                out.push(data.join("\n"));
            }
        }
        out
    }
}

/// What an `/api/me/events` frame means to the watcher.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A notification landed: read the rows.
    Notification,
    /// The unread count changed (I-9, `{"type":"notifications","unread":N}`):
    /// the badge share, with no read.
    Unread(u32),
    Other,
}

pub fn frame(data: &str) -> Frame {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return Frame::Other;
    };
    match value.get("type").and_then(|t| t.as_str()) {
        Some("notification") => Frame::Notification,
        Some("notifications") => value
            .get("unread")
            .and_then(serde_json::Value::as_u64)
            .map_or(Frame::Other, |n| {
                Frame::Unread(u32::try_from(n).unwrap_or(u32::MAX))
            }),
        _ => Frame::Other,
    }
}

/// The `Cookie` header for a request to the instance: every cookie its
/// webview holds for that URL, as the webview would send it.
pub fn cookie_header<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<String> {
    let header = pairs
        .into_iter()
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    (!header.is_empty()).then_some(header)
}

/// The reconnect delay: doubling from the floor to the cap, with up to a
/// tenth more at random so many shells do not reconnect in step after a
/// deploy.
#[derive(Debug)]
pub struct Backoff {
    next: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            next: BACKOFF_FLOOR,
        }
    }
}

impl Backoff {
    /// The delay before the next attempt, without the jitter.
    pub fn step(&mut self) -> Duration {
        let now = self.next;
        self.next = (self.next * 2).min(BACKOFF_CAP);
        now
    }

    pub fn reset(&mut self) {
        self.next = BACKOFF_FLOOR;
    }

    fn jittered(&mut self) -> Duration {
        let base = self.step();
        let spread = (uuid::Uuid::new_v4().as_u128() % 1_000) as f64 / 10_000.0;
        base.mul_f64(1.0 + spread)
    }
}

enum Fetch {
    Rows(Vec<Row>, Option<u64>),
    SignedOut,
    Failed,
}

/// The instance itself said the session is gone: a 401 or 403 with its own
/// JSON body. A Cloudflare challenge (`cf-mitigated`, an HTML page) or any
/// other proxy's refusal is an outage like a 502 — retried with the poll
/// floor, never read as signed out, or an instance behind Cloudflare would
/// stop its banners for good on the first challenge.
pub fn signed_out(status: StatusCode, headers: &HeaderMap) -> bool {
    matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN)
        && !headers.contains_key("cf-mitigated")
        && headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("application/json")
            })
}

async fn fetch(client: &reqwest::Client, origin: &str, cookie: &str) -> Fetch {
    let response = client
        .get(format!("{origin}/api/notifications"))
        .header(COOKIE, cookie)
        .header(ACCEPT, "application/json")
        .send()
        .await;
    match response {
        Ok(r) if signed_out(r.status(), r.headers()) => Fetch::SignedOut,
        Ok(r) if r.status().is_success() => match r.json::<Payload>().await {
            Ok(p) => Fetch::Rows(p.notifications, p.unread),
            Err(_) => Fetch::Failed,
        },
        _ => Fetch::Failed,
    }
}

fn client() -> reqwest::Client {
    crate::build_client(|b| {
        b.read_timeout(READ_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
    })
}

/// What the shell's first read of an instance's notifications found, taken
/// while its page is still open.
pub enum Baseline {
    /// The rows the page has already surfaced: the watcher starts from them.
    Ready(Arrivals),
    /// The instance's own 401 or 403: the page is signed out and is not
    /// fetching either.
    SignedOut,
    /// Anything else: a proxy or a certificate only the webview knows, the
    /// instance down. The shell cannot do the page's job, so the page stays.
    Unreachable,
}

/// The longest the first read may take. The page waits open for it, and a
/// read that has not finished by then counts as one the shell cannot make.
pub const BASELINE_WAIT: Duration = Duration::from_secs(30);

/// One read of `/api/notifications` as the page would make it, bounded by
/// `BASELINE_WAIT`.
pub async fn baseline(origin: &str, cookie: &str) -> Baseline {
    let read = tokio::time::timeout(BASELINE_WAIT, fetch(&client(), origin, cookie)).await;
    match read.unwrap_or(Fetch::Failed) {
        Fetch::Rows(rows, _) => {
            let mut arrivals = Arrivals::default();
            arrivals.arrive(&rows);
            Baseline::Ready(arrivals)
        }
        Fetch::SignedOut => Baseline::SignedOut,
        Fetch::Failed => Baseline::Unreachable,
    }
}

/// Who the watcher works for.
pub struct Watch {
    pub app: AppHandle,
    /// `instance-<id>`.
    pub label: String,
    /// The instance's display label, for the banner.
    pub instance: String,
    pub origin: String,
    pub cookie: String,
    pub arrivals: Arrivals,
    pub shared: Shared,
}

/// Start watching. The handle aborts it.
pub fn spawn(watch: Watch) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(run(watch))
}

struct SignedOut;

async fn run(mut watch: Watch) {
    let client = client();
    let label = watch.label.clone();
    eprintln!("talaria-desktop: {label}: watching notifications while suspended");
    let mut backoff = Backoff::default();
    // The first read is now: with the page's record, it posts what the page
    // missed.
    let mut next_poll = Instant::now();
    loop {
        let stream = client
            .get(format!("{}/api/me/events", watch.origin))
            .header(COOKIE, &watch.cookie)
            .header(ACCEPT, "text/event-stream")
            .send()
            .await;
        let mut response = match stream {
            Ok(r) if signed_out(r.status(), r.headers()) => {
                eprintln!("talaria-desktop: {label}: signed out; no notifications while suspended");
                return;
            }
            Ok(r) if r.status().is_success() => Some(r),
            Ok(r) => {
                eprintln!(
                    "talaria-desktop: {label}: the event stream answered {}",
                    r.status()
                );
                None
            }
            Err(e) => {
                eprintln!("talaria-desktop: {label}: the event stream: {e}");
                None
            }
        };
        if let Some(r) = response.as_mut() {
            eprintln!("talaria-desktop: {label}: event stream open");
            // A reopened stream may have missed something: read now.
            next_poll = Instant::now();
            let opened = Instant::now();
            let mut frames = Frames::default();
            loop {
                let read = tokio::time::timeout_at(next_poll, r.chunk()).await;
                let check = match read {
                    Ok(Ok(Some(bytes))) => {
                        let mut check = false;
                        for data in frames.push(&bytes) {
                            match frame(&data) {
                                Frame::Notification => check = true,
                                Frame::Unread(n) => {
                                    crate::notify::set_badge(&watch.app, &label, n);
                                }
                                Frame::Other => {}
                            }
                        }
                        check
                    }
                    // Ended or broke: reconnect with backoff.
                    Ok(_) => break,
                    // The poll floor.
                    Err(_) => true,
                };
                if !check {
                    continue;
                }
                if Instant::now() >= next_poll {
                    next_poll = Instant::now() + POLL;
                }
                if read_and_post(&mut watch, &client).await.is_err() {
                    return;
                }
            }
            if opened.elapsed() >= HEALTHY {
                backoff.reset();
            }
        }
        let delay = backoff.jittered();
        eprintln!(
            "talaria-desktop: {label}: the event stream closed; reconnecting in {:.2} s",
            delay.as_secs_f64()
        );
        let reconnect_at = Instant::now() + delay;
        // The poll floor keeps running through the outage.
        while next_poll < reconnect_at {
            tokio::time::sleep_until(next_poll).await;
            next_poll = Instant::now() + POLL;
            eprintln!("talaria-desktop: {label}: poll floor read while the stream is down");
            if read_and_post(&mut watch, &client).await.is_err() {
                return;
            }
        }
        tokio::time::sleep_until(reconnect_at).await;
    }
}

/// One read of the rows: the badge share, then every arrival posted.
async fn read_and_post(watch: &mut Watch, client: &reqwest::Client) -> Result<(), SignedOut> {
    match fetch(client, &watch.origin, &watch.cookie).await {
        Fetch::Rows(rows, unread) => {
            if let Some(unread) = unread {
                crate::notify::set_badge(
                    &watch.app,
                    &watch.label,
                    u32::try_from(unread).unwrap_or(u32::MAX),
                );
            }
            let fresh = watch.arrivals.arrive(&rows);
            post_all(watch, fresh).await;
            Ok(())
        }
        Fetch::SignedOut => {
            eprintln!(
                "talaria-desktop: {}: signed out; no notifications while suspended",
                watch.label
            );
            Err(SignedOut)
        }
        Fetch::Failed => Ok(()),
    }
}

/// The summary banner for a burst: "N new in <label>".
pub fn summary_title(n: usize, instance: &str) -> String {
    format!("{n} new in {instance}")
}

async fn post_all(watch: &Watch, fresh: Vec<Arrival>) {
    if fresh.is_empty() {
        return;
    }
    {
        let mut shared = watch.shared.lock().unwrap();
        for arrival in &fresh {
            shared
                .posted
                .push((arrival.row.id.clone(), arrival.row.created_text()));
        }
    }
    if fresh.len() > SUMMARY_OVER {
        eprintln!(
            "talaria-desktop: {}: posting a summary of {} while suspended",
            watch.label,
            fresh.len()
        );
        let newest: Vec<String> = fresh
            .iter()
            .rev()
            .take(SUMMARY_OVER)
            .map(|a| crate::notify::clamped_title(&a.row.title))
            .filter(|t| !t.is_empty())
            .collect();
        let post = crate::notify::Post {
            label: watch.label.clone(),
            instance: watch.instance.clone(),
            title: summary_title(fresh.len(), &watch.instance),
            body: Some(crate::notify::clamped_body(&newest.join(" · "))).filter(|b| !b.is_empty()),
            tag: "summary".into(),
            href: None,
            kind: None,
            renotify: true,
            source: crate::notify::Source::Page,
        };
        if let Err(e) = crate::notify::post(&watch.app, post).await {
            eprintln!("talaria-desktop: {}: {e}", watch.label);
        }
        return;
    }
    for arrival in fresh {
        let row = arrival.row;
        let title = crate::notify::clamped_title(&row.title);
        if title.is_empty() {
            continue;
        }
        eprintln!(
            "talaria-desktop: {}: posting {} while suspended{}",
            watch.label,
            row.id,
            if arrival.realert { " (re-alert)" } else { "" }
        );
        let post = crate::notify::Post {
            label: watch.label.clone(),
            instance: watch.instance.clone(),
            title,
            body: Some(crate::notify::clamped_body(&row.body)).filter(|b| !b.is_empty()),
            tag: crate::notify::clamped_tag(&row.id),
            href: crate::notify::openable_href(&row.href).map(str::to_string),
            kind: crate::notify::clean_kind(&row.kind),
            renotify: arrival.realert,
            source: crate::notify::Source::Watcher {
                row: row.id.clone(),
                origin: watch.origin.clone(),
                cookie: watch.cookie.clone(),
            },
        };
        if let Err(e) = crate::notify::post(&watch.app, post).await {
            eprintln!("talaria-desktop: {}: {e}", watch.label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_793_000_000_000;

    fn row_at(id: &str, ms: i64) -> Row {
        Row {
            id: id.into(),
            kind: "dm".into(),
            title: format!("t-{id}"),
            body: String::new(),
            href: String::new(),
            read_at: None,
            created_at: serde_json::Value::String(iso_of_ms(ms)),
        }
    }

    fn ids(arrivals: &[Arrival]) -> Vec<&str> {
        arrivals.iter().map(|a| a.row.id.as_str()).collect()
    }

    #[test]
    fn the_first_read_is_the_baseline_then_each_arrival_once_oldest_first() {
        let mut a = Arrivals::default();
        assert!(
            a.arrive(&[row_at("2", T0 + 2), row_at("1", T0 + 1)])
                .is_empty()
        );
        let fresh = a.arrive(&[
            row_at("4", T0 + 4_000),
            row_at("3", T0 + 3_000),
            row_at("2", T0 + 2),
            row_at("1", T0 + 1),
        ]);
        assert_eq!(ids(&fresh), vec!["3", "4"]);
        assert!(fresh.iter().all(|a| !a.realert));
        assert!(
            a.arrive(&[row_at("4", T0 + 4_000), row_at("3", T0 + 3_000)])
                .is_empty()
        );
        // An empty baseline still primes: the first row ever is an arrival.
        let mut b = Arrivals::default();
        assert!(b.arrive(&[]).is_empty());
        assert_eq!(ids(&b.arrive(&[row_at("1", T0)])), vec!["1"]);
    }

    #[test]
    fn a_read_row_or_one_older_than_the_margin_is_never_an_arrival() {
        let mut a = Arrivals::default();
        a.arrive(&[row_at("1", T0)]);
        let mut read = row_at("2", T0 + 5_000);
        read.read_at = Some(serde_json::Value::String(iso_of_ms(T0 + 6_000)));
        let old = row_at("3", T0 - MARGIN_MS - 1);
        let late_but_inside = row_at("4", T0 - MARGIN_MS + 1_000);
        assert_eq!(ids(&a.arrive(&[read, old, late_but_inside])), vec!["4"]);
    }

    #[test]
    fn a_known_row_whose_created_at_moved_later_is_a_re_alert() {
        let mut a = Arrivals::default();
        a.arrive(&[row_at("dm-1", T0)]);
        let fresh = a.arrive(&[row_at("dm-1", T0 + 60_000)]);
        assert_eq!(ids(&fresh), vec!["dm-1"]);
        assert!(fresh[0].realert);
        // Seen at the new time now: not again.
        assert!(a.arrive(&[row_at("dm-1", T0 + 60_000)]).is_empty());
    }

    #[test]
    fn the_pages_record_makes_its_missed_rows_arrivals_on_the_first_read() {
        // The page surfaced 1, its stream died, 2 landed, then it was
        // suspended: the handoff says what it had seen.
        let handoff = Handoff {
            user: "u1".into(),
            hw: Some(iso_of_ms(T0)),
            seen: [("1".to_string(), iso_of_ms(T0))].into_iter().collect(),
            at: Some(1.0),
        };
        let mut a = Arrivals::from_handoff(&handoff);
        let fresh = a.arrive(&[row_at("2", T0 + 30_000), row_at("1", T0)]);
        assert_eq!(ids(&fresh), vec!["2"]);
        assert!(
            a.arrive(&[row_at("2", T0 + 30_000), row_at("1", T0)])
                .is_empty()
        );
        // Without the handoff, the same read would have been swallowed (R13).
        let mut fresh_baseline = Arrivals::default();
        assert!(
            fresh_baseline
                .arrive(&[row_at("2", T0 + 30_000), row_at("1", T0)])
                .is_empty()
        );
    }

    #[test]
    fn a_handoff_is_bounded_and_needs_a_user() {
        assert!(Handoff::default().clean().is_none());
        let big = Handoff {
            user: "u1".into(),
            hw: None,
            seen: (0..(MAX_SEEN as i64 + 50))
                .map(|i| (format!("r{i}"), iso_of_ms(T0 + i)))
                .collect(),
            at: None,
        }
        .clean()
        .unwrap();
        assert_eq!(big.seen.len(), MAX_SEEN);
        // The newest are the ones kept.
        assert!(big.seen.contains_key(&format!("r{}", MAX_SEEN as i64 + 49)));
        assert!(!big.seen.contains_key("r0"));
        let parsed: Handoff = serde_json::from_str(
            r#"{"user":"u1","hw":"2026-10-31T10:00:00.000Z","seen":{"a":"2026-10-31T10:00:00.000Z"},"at":1793000000000}"#,
        )
        .unwrap();
        assert_eq!(parsed.seen.len(), 1);
        let lenient = Handoff::from_value(&serde_json::json!({
            "user": "u1",
            "hw": 7,
            "seen": {"a": "2026-10-31T10:00:00.000Z", "b": 3, "c": null},
            "at": "x"
        }))
        .unwrap();
        assert_eq!(lenient.seen.len(), 1);
        assert_eq!((lenient.hw, lenient.at), (None, None));
        assert!(Handoff::from_value(&serde_json::json!({"seen": {}})).is_none());
        assert!(Handoff::from_value(&serde_json::json!("nope")).is_none());
    }

    #[test]
    fn the_remembered_set_stays_bounded() {
        let mut a = Arrivals::default();
        a.arrive(&[]);
        for i in 0..(MAX_SEEN as i64 + 10) {
            a.arrive(&[row_at(&i.to_string(), T0 + i * 1_000)]);
        }
        assert!(a.seen.len() <= MAX_SEEN);
        assert!(a.seen.contains_key(&(MAX_SEEN as i64 + 9).to_string()));
    }

    #[test]
    fn iso_timestamps_round_trip_and_junk_is_none() {
        for ms in [0, T0, T0 + 999, 951_782_400_000 /* 2000-02-29 */] {
            assert_eq!(parse_iso_ms(&iso_of_ms(ms)), Some(ms), "{ms}");
        }
        assert_eq!(iso_of_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            parse_iso_ms("2026-10-31T12:00:00+02:00"),
            parse_iso_ms("2026-10-31T10:00:00Z")
        );
        assert_eq!(
            parse_iso_ms("2026-10-31T10:00:00.5Z"),
            parse_iso_ms("2026-10-31T10:00:00.500Z")
        );
        for junk in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-10-31",
            "2026-10-31T10:00:00Q",
            "2026-10-31T10:00:00.Z",
        ] {
            assert_eq!(parse_iso_ms(junk), None, "{junk:?}");
        }
        let numeric = Row {
            created_at: serde_json::json!(T0),
            ..row_at("x", 0)
        };
        assert_eq!(numeric.created_ms(), Some(T0));
        assert_eq!(numeric.created_text(), iso_of_ms(T0));
    }

    #[test]
    fn the_backoff_doubles_to_the_cap_and_resets() {
        let mut b = Backoff::default();
        let steps: Vec<u64> = (0..7).map(|_| b.step().as_secs()).collect();
        assert_eq!(steps, vec![1, 2, 4, 8, 16, 30, 30]);
        b.reset();
        assert_eq!(b.step(), Duration::from_secs(1));
        let jittered = Backoff::default().jittered();
        assert!(jittered >= Duration::from_secs(1) && jittered <= Duration::from_millis(1_100));
    }

    #[test]
    fn frames_split_events_across_chunks_and_skip_comments() {
        let mut f = Frames::default();
        assert!(f.push(b": connected\n\n").is_empty());
        assert!(f.push(b"data: {\"type\":\"notif").is_empty());
        assert_eq!(
            f.push(
                b"ication\",\"notificationId\":\"n1\"}\n\ndata: {\"type\":\"run\"}\r\n\r\n: ping\n\n"
            ),
            vec![
                r#"{"type":"notification","notificationId":"n1"}"#.to_string(),
                r#"{"type":"run"}"#.to_string()
            ]
        );
        assert_eq!(
            frame(r#"{"type":"notification","notificationId":"n1"}"#),
            Frame::Notification
        );
        assert_eq!(
            frame(r#"{"type":"notifications","unread":4}"#),
            Frame::Unread(4)
        );
        assert_eq!(frame(r#"{"type":"notifications"}"#), Frame::Other);
        assert_eq!(frame(r#"{"type":"run","runId":"r"}"#), Frame::Other);
        assert_eq!(frame("not json"), Frame::Other);
    }

    #[test]
    fn frames_keep_a_character_or_a_crlf_split_across_chunks() {
        let event = "data: {\"type\":\"notification\",\"title\":\"Zoë — 日本\"}\r\n\r\n".as_bytes();
        // Every split point, including inside the multibyte characters and
        // between a CR and its LF.
        for at in 1..event.len() {
            let mut f = Frames::default();
            let mut out = f.push(&event[..at]);
            out.extend(f.push(&event[at..]));
            assert_eq!(
                out,
                vec![r#"{"type":"notification","title":"Zoë — 日本"}"#.to_string()],
                "split at {at}"
            );
        }
        // A lone CR is a line ending too.
        let mut f = Frames::default();
        assert_eq!(f.push(b"data: a\r\rdata: b\n\n"), vec!["a", "b"]);
    }

    #[test]
    fn only_the_instances_own_refusal_is_signed_out() {
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut h = HeaderMap::new();
            for (k, v) in pairs {
                h.insert(*k, v.parse().unwrap());
            }
            h
        };
        let json = headers(&[("content-type", "application/json")]);
        assert!(signed_out(StatusCode::UNAUTHORIZED, &json));
        assert!(signed_out(StatusCode::FORBIDDEN, &json));
        assert!(signed_out(
            StatusCode::FORBIDDEN,
            &headers(&[("content-type", "Application/JSON; charset=utf-8")])
        ));
        // A Cloudflare managed challenge: 403, cf-mitigated, an HTML page.
        assert!(!signed_out(
            StatusCode::FORBIDDEN,
            &headers(&[
                ("content-type", "text/html; charset=UTF-8"),
                ("cf-mitigated", "challenge")
            ])
        ));
        assert!(!signed_out(
            StatusCode::FORBIDDEN,
            &headers(&[
                ("content-type", "application/json"),
                ("cf-mitigated", "challenge")
            ])
        ));
        assert!(!signed_out(
            StatusCode::FORBIDDEN,
            &headers(&[("content-type", "text/html")])
        ));
        assert!(!signed_out(StatusCode::FORBIDDEN, &HeaderMap::new()));
        assert!(!signed_out(StatusCode::BAD_GATEWAY, &json));
        assert!(!signed_out(StatusCode::OK, &json));
    }

    #[test]
    fn the_payload_parses_the_stable_row_shape() {
        let p: Payload = serde_json::from_str(
            r#"{"notifications":[{"id":"n1","kind":"dm","title":"t","body":"b","href":"/c","readAt":null,"createdAt":"2026-10-31T10:00:00.000Z"}],"unread":3,"prompt":{"x":1}}"#,
        )
        .unwrap();
        assert_eq!(p.unread, Some(3));
        assert!(p.notifications[0].unread());
        assert_eq!(p.notifications[0].kind, "dm");
        let older: Payload = serde_json::from_str(r#"{"notifications":[{"id":"n1"}]}"#).unwrap();
        assert_eq!(older.unread, None);
        assert_eq!(older.notifications[0].created_ms(), None);
    }

    #[test]
    fn the_cookie_header_joins_every_pair() {
        assert_eq!(
            cookie_header([("talaria_session", "abc%2F"), ("other", "1")]),
            Some("talaria_session=abc%2F; other=1".into())
        );
        assert_eq!(cookie_header([]), None);
        assert_eq!(cookie_header([("", "x")]), None);
    }

    #[test]
    fn a_burst_is_one_summary() {
        assert_eq!(summary_title(5, "Acme"), "5 new in Acme");
    }
}
