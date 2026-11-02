//! An instance's notifications, while no page of it is open.
//!
//! An open page posts its own: ui's `NotificationToasts` watches
//! `GET /api/notifications` (made live by `/api/me/events`, with a 30 s poll
//! floor) and, for every row that lands while Talaria is not what the person
//! is looking at, asks the shell to post it (`desktop_notify`). A suspended
//! page — or one the shell loaded hidden at launch and then let go, so a
//! signed-in instance nobody opened still alerts (D8) — posts nothing, so the
//! shell does the same job for it, natively: the same two routes, the same
//! arrival rule, the same banner and the same click.
//!
//! The arrival rule is the page tracker's (I-20, wave 1's I-3 v2). A row that
//! carries a `seq`, read by a watcher whose record has one, is an arrival when
//! it is unread, its seq is past the record's, and its id is unseen or its
//! `createdAt` moved later (a re-alert, I-8). Any other row keeps wave 1's
//! rule: unread, newer than the high-water mark minus two minutes, unseen or
//! later. With `since` in the instance's `features`, the watcher reads
//! `GET /api/notifications?since=<seq>`, page after page, and the record's
//! seq advances to the snapshot of a page whose `more` is false — or, when a
//! long catch-up runs out of its ten pages, to the last row read, so the next
//! read goes on from there — so a reconnect after an outage, a sleep or a cut
//! stream is exact, never "the newest 50". The
//! watcher starts from the PAGE's record when the page hands one over in its
//! suspend report, so whatever the page missed is posted on the watcher's
//! first read (D2); with none (an older instance UI), the first read is the
//! baseline. More than three arrivals at once are one summary banner, tagged
//! `talaria-summary` like the page's (I-21); its click restores the instance
//! and hands the page `talaria:notification-open {id:'talaria-summary'}`,
//! which opens the bell.
//!
//! What it posts (I-15): never a row whose `alert` is `feed`; while the
//! person's Do Not Disturb is active, only rows of a class that breaks
//! through it; nothing at all for an instance muted in the launcher; and
//! nothing while the person's presence winner (I-17) is another of their
//! clients — a browser tab they are looking at, another desktop — which
//! alerts instead, as the page runtime's cross-device rule has it, so one
//! device alerts, not two. Every row it does post is acked (`POST
//! /api/notifications/ack`, I-18), so the server's deferred push for it is
//! cancelled: the person was told here.
//!
//! Read-sync (I-16): a `notifications` frame names the rows read elsewhere
//! (`readIds`, or `all`), and the watcher closes its own banners for them —
//! keyed by this instance's label, so another instance's read never closes
//! this one's. `resync` (too many to name) is a list read that closes every
//! banner whose row is read now.
//!
//! Presence (I-17, I-22): every 30 s it heartbeats `PUT /api/me/presence` as
//! `desk-<instance id>`, kind `desktop`, `active` while the OS saw input in
//! the last two minutes, else `idle` — so the instance's push waits while
//! the person is at this desktop, and does not while they are away. Each
//! beat's answer, and each `presence` frame, names the winner.
//!
//! It runs only when the page said it would post (`notify` in its report: ui's
//! `browserNotifyEnabled`), with that instance's own cookies, read from its
//! webview just before it closed, and only ever to that instance's origin:
//! redirects are not followed. A stream that fails — a 502 while the instance
//! restarts, a deploy, Cloudflare dropping an idle connection — is reopened
//! with backoff (1 s doubling to 30 s, reset after a minute healthy), with
//! `?since=<seq>` so the server replays what the cut lost (I-19), every
//! reopen refetches, and the 30 s poll floor keeps running through the whole
//! outage. A 401 or 403 from the instance itself (signed out, session
//! expired: its JSON, not a Cloudflare challenge page) ends it; a restore ends
//! it too, and hands back what it saw and how far it read (`Handback`), so the
//! restored page never posts it again. Each read, and each `notifications`
//! frame, sets the instance's share of the badge: For you
//! (`unreadForYou`), or `unread` from an instance that predates it.
//!
//! An older instance (no `features`) gets wave 1's watcher exactly: list
//! reads, wave 1's rule, no ack, no heartbeat.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{
    StatusCode,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, HeaderMap, ORIGIN},
};
use serde::Deserialize;
use tauri::AppHandle;
use tokio::time::Instant;

/// The poll floor: the page's own `refetchInterval`.
const POLL: Duration = Duration::from_secs(30);

/// The presence heartbeat (I-17): well inside the store's 75 s staleness.
const BEAT: Duration = Duration::from_secs(30);

/// OS input this recent is "active" (O-13: the desktop's two minutes).
pub const ACTIVE_MS: u64 = 120_000;

/// No bytes for this long and the stream is presumed dead. The server pings
/// every 25 s.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// The bound on the watcher's small writes: an ack, a heartbeat.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// The reconnect backoff: from this, doubling, up to the cap; a stream that
/// stayed up this long starts over at the floor.
const BACKOFF_FLOOR: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);
const HEALTHY: Duration = Duration::from_secs(60);

/// The most row ids remembered: the page's tracker keeps the same.
pub const MAX_SEEN: usize = 300;

/// A row this much older than the high-water mark is never an arrival
/// (wave 1's rule, for rows and records with no seq).
pub const MARGIN_MS: i64 = 120_000;

/// More arrivals than this in one read are one summary banner.
pub const SUMMARY_OVER: usize = 3;

/// The catch-up summary's tag, the page's and the watcher's (I-21).
pub const SUMMARY_TAG: &str = "talaria-summary";

/// `?since` pages read in one go (200 rows each), the page runtime's budget.
/// Past it, the cursor moves to the last row read — a page is complete up to
/// its last row (I-10) — and the next read goes on from there.
pub const MAX_SINCE_PAGES: usize = 10;

/// The ack's own bound per request (the route refuses more).
const ACK_CHUNK: usize = 200;

/// The most read ids one frame may name (the server's own bound).
const MAX_READ_IDS: usize = 100;

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
    /// The person's notify seq this write took (I-10); none on a row filed
    /// before the contract, or from an instance that predates it.
    #[serde(default, deserialize_with = "lenient_seq")]
    pub seq: Option<i64>,
    /// `banner` or `feed` (I-15); none means banner.
    #[serde(default, deserialize_with = "lenient_string")]
    pub alert: Option<String>,
    /// The row's class (I-12), for Do Not Disturb's break-through.
    #[serde(default, deserialize_with = "lenient_string")]
    pub class: Option<String>,
}

/// A value this build cannot read as a whole number is no seq.
fn lenient_seq<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(value.as_i64().filter(|n| *n >= 0))
}

fn lenient_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(value.as_str().map(str::to_string))
}

impl Row {
    fn unread(&self) -> bool {
        self.read_at.as_ref().is_none_or(serde_json::Value::is_null)
    }

    /// Recorded, never bannered (I-15).
    pub fn feed(&self) -> bool {
        self.alert.as_deref() == Some("feed")
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

/// What the instance told the watcher about itself on its last read: which
/// contract features it serves (I-23), and whether the person's Do Not
/// Disturb is on and what breaks through it (I-14, I-15). An instance that
/// serves no `features` is wave 1's, and this stays empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Server {
    pub features: HashSet<String>,
    pub dnd_active: bool,
    /// `dnd.allowNeedsYou`: off, nothing breaks through.
    pub allow_needs_you: bool,
    /// The classes whose rows alert through Do Not Disturb.
    pub breaks: HashSet<String>,
}

impl Server {
    pub fn has(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    /// May this row be bannered now (I-15)? Never a feed row; while Do Not
    /// Disturb is active, only a row of a break-through class.
    pub fn bannerable(&self, row: &Row) -> bool {
        if row.feed() {
            return false;
        }
        if !self.dnd_active {
            return true;
        }
        self.allow_needs_you
            && row
                .class
                .as_deref()
                .is_some_and(|c| self.breaks.contains(c))
    }

    /// The payload's own word, read leniently: a key this build cannot read
    /// is the default, never a failed read.
    fn of(payload: &serde_json::Value) -> Self {
        let features: HashSet<String> = payload
            .get("features")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let dnd = payload.get("dnd");
        let dnd_active = dnd
            .and_then(|d| d.get("active"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let allow_needs_you = dnd
            .and_then(|d| d.get("allowNeedsYou"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let breaks = payload
            .get("classes")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter(|c| {
                        c.get("breaksThrough")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false)
                    })
                    .filter_map(|c| c.get("id").and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            features,
            dnd_active,
            allow_needs_you,
            breaks,
        }
    }
}

/// One read of `/api/notifications`, plain or `?since`.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub rows: Vec<Row>,
    /// This instance's share of the badge: For you, or every unread row from
    /// an instance that predates For you (EB-11, O-10).
    pub badge: Option<u32>,
    /// The top-level seq (I-10): a snapshot, read with the rows.
    pub seq: Option<i64>,
    /// A `?since` page that is not the last.
    pub more: bool,
    pub server: Server,
}

/// A payload as the watcher reads it. Rows that do not parse are skipped,
/// one at a time; a body that is not an object is no read at all.
pub fn parse_read(payload: &serde_json::Value) -> Option<Read> {
    let object = payload.as_object()?;
    let rows = object
        .get("notifications")
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| serde_json::from_value::<Row>(r.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let count = |key: &str| {
        object
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    Some(Read {
        rows,
        badge: count("unreadForYou").or_else(|| count("unread")),
        seq: object
            .get("seq")
            .and_then(serde_json::Value::as_i64)
            .filter(|n| *n >= 0),
        more: object
            .get("more")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        server: Server::of(payload),
    })
}

/// How far one catch-up of `?since` pages has judged (I-20). The snapshot
/// seq of a page whose `more` is false is a read-through; so, when the page
/// budget or a failed page stops a catch-up short, is the last row of the
/// last page read, because a page is complete up to its last row (I-10:
/// every row at or below a seen seq had committed). Without the second rule
/// a catch-up longer than the budget would start over on every read: the
/// rows the seen set has since let go of would be posted again each time,
/// and the rows past the budget never.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SincePages {
    /// The read-through the pages so far establish.
    through: Option<i64>,
}

impl SincePages {
    /// Take one page; the cursor for the next, or None when this one ends
    /// the catch-up (the last page, or a `more` with no row to go on from).
    pub fn page(&mut self, read: &Read) -> Option<i64> {
        let end = read.rows.iter().filter_map(|r| r.seq).max();
        match (read.more, end) {
            (false, _) => {
                if let Some(seq) = read.seq {
                    self.advance(seq);
                }
                None
            }
            (true, Some(end)) => {
                self.advance(end);
                Some(end)
            }
            (true, None) => None,
        }
    }

    fn advance(&mut self, seq: i64) {
        self.through = Some(self.through.map_or(seq, |t| t.max(seq)));
    }

    /// The seq every row at or below has been judged, if any page said.
    pub fn through(&self) -> Option<i64> {
        self.through
    }
}

/// The server's seq, when a complete `?since` page says it is BELOW the
/// cursor the watcher read from: the instance's database was restored or
/// reset under it (a person's seq never goes back otherwise, I-10). Every row
/// filed since then has a seq at or below the old cursor, so reading on from
/// it would surface nothing until the server passed it again. The caller
/// rebases (`Arrivals::rebase`) instead, as the page does (W2-R).
pub fn server_behind(cursor: Option<i64>, read: &Read) -> Option<i64> {
    match (cursor, read.more, read.seq) {
        (Some(cursor), false, Some(seq)) if seq < cursor => Some(seq),
        _ => None,
    }
}

/// Whether a `resync` frame asks for a read: an incomplete replay, a seq past
/// the one read through, or one BELOW it (a restore; the read then rebases).
pub fn resync_needs_read(seq: i64, complete: bool, through: Option<i64>) -> bool {
    !complete || through.is_none_or(|through| seq != through)
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

/// The page's arrival record for one user (I-20,
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
    /// The notify seq the page had read THROUGH (I-20 v2), when it had one.
    #[serde(default)]
    pub seq: Option<i64>,
}

impl Handoff {
    /// The record as the suspend report carried it, read leniently: entries
    /// that are not strings are skipped, a seq that is not a whole number is
    /// none, and a record this build cannot read at all is none.
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
        let seq = value
            .get("seq")
            .and_then(serde_json::Value::as_i64)
            .filter(|n| *n >= 0);
        Self {
            user,
            hw,
            seen,
            at,
            seq,
        }
        .clean()
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

/// The arrival tracker (I-20's rule; see the module docs).
#[derive(Debug, Default, Clone)]
pub struct Arrivals {
    hw: Option<i64>,
    seen: HashMap<String, i64>,
    primed: bool,
    /// The seq read THROUGH: every row at or below it has been judged.
    seq: Option<i64>,
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
            seq: handoff.seq,
        }
    }

    /// The seq read through, if any read has established one.
    pub fn seq(&self) -> Option<i64> {
        self.seq
    }

    /// Every row at or below `seq` has been judged: a complete `?since` read
    /// (or the baseline, which by definition surfaces nothing before it).
    /// Never moves backwards.
    pub fn read_through(&mut self, seq: i64) {
        self.seq = Some(self.seq.map_or(seq, |s| s.max(seq)));
    }

    /// The server's seq fell below the record's (`server_behind`): judge the
    /// newest rows by wave 1's rule — the high-water mark minus the margin,
    /// and the seen set — so a row filed since the restore surfaces and one
    /// from before it does not, then take the server's seq as the cursor. The
    /// one place the cursor moves back.
    pub fn rebase(&mut self, rows: &[Row], seq: i64) -> Vec<Arrival> {
        self.seq = None;
        let fresh = self.arrive(rows);
        self.seq = Some(seq);
        fresh
    }

    /// Rows to surface now, oldest first. `rows` may come in any order (a
    /// plain read is newest first, a `?since` read is in seq order). The
    /// first read of an unprimed tracker is the baseline, empty or not.
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
            if !row.unread() {
                continue;
            }
            match (row.seq, self.seq) {
                // I-20: past the record's seq, and no margin.
                (Some(seq), Some(through)) => {
                    if seq <= through {
                        continue;
                    }
                }
                // Wave 1's rule: the high-water mark minus the margin.
                _ => {
                    if at <= floor {
                        continue;
                    }
                }
            }
            let realert = match self.seen.get(&row.id) {
                None => false,
                Some(seen) if at > *seen => true,
                Some(_) => continue,
            };
            if fresh.iter().any(|a: &Arrival| a.row.id == row.id) {
                continue;
            }
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
        fresh.sort_by_key(|a| (a.row.created_ms().unwrap_or(i64::MIN), a.row.seq));
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

/// What the watcher saw, for the restore to merge into the page's record
/// (I-20) so the restored page never surfaces it again.
#[derive(Debug, Default)]
pub struct Handback {
    /// The record's user, from the page's handoff. None: the page keeps no
    /// record (an older UI), and there is nothing to merge into.
    pub user: Option<String>,
    /// Row id → `createdAt` as judged: every arrival, posted, held by Do Not
    /// Disturb, muted or feed alike — the page would not banner them either.
    pub posted: Vec<(String, String)>,
    /// The seq the watcher read through, for the record's own (I-20).
    pub seq: Option<i64>,
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

/// A read-sync frame (I-9, I-16): the counts, and which rows were read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadSync {
    pub unread: u32,
    pub unread_for_you: Option<u32>,
    /// The rows just read, at most 100.
    pub read_ids: Vec<String>,
    /// Everything was read (mark all read).
    pub all: bool,
    /// Too many to name: what is read now is to be looked up.
    pub resync: bool,
}

impl ReadSync {
    /// This instance's share of the badge: For you, or every unread row.
    pub fn badge(&self) -> u32 {
        self.unread_for_you.unwrap_or(self.unread)
    }
}

/// What an `/api/me/events` frame means to the watcher.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    /// A notification landed, with the seq its write took (I-19): read the
    /// rows, unless that seq is one already read through.
    Notification(Option<i64>),
    /// The counts changed, and maybe which rows were read.
    Notifications(ReadSync),
    /// The server's replay state (I-19): `complete:false` means read with
    /// `?since` now.
    Resync {
        seq: i64,
        complete: bool,
    },
    /// The session behind the stream ended; the stream closes after it.
    SessionEnded,
    /// The person's presence winner changed (I-17).
    Presence(Winner),
    Other,
}

/// The presence winner (I-17): the client the person is looking at, else
/// the one they are active on — a client id each, or none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Winner {
    pub looking: Option<String>,
    pub active: Option<String>,
}

impl Winner {
    /// The winner pair from a `presence` frame, a heartbeat's answer or
    /// `GET /api/me/presence`: the same two keys. None for a body without
    /// either key (an instance that predates the presence store).
    pub fn of(value: &serde_json::Value) -> Option<Self> {
        if value.get("looking").is_none() && value.get("active").is_none() {
            return None;
        }
        let id = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            looking: id("looking"),
            active: id("active"),
        })
    }

    /// The client the person is on, when it is none of `own` (this
    /// watcher's client id, and the instance's page on this desktop): the
    /// page runtime's cross-device rule, so only the device the person is on
    /// alerts. None when nobody wins (the person is idle everywhere: every
    /// client keeps its own rule) or this desktop does.
    pub fn elsewhere(&self, own: &[String]) -> Option<&str> {
        self.looking
            .as_deref()
            .or(self.active.as_deref())
            .filter(|id| !own.iter().any(|o| o == id))
    }
}

pub fn frame(data: &str) -> Frame {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return Frame::Other;
    };
    let count = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
    };
    let flag = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    match value.get("type").and_then(|t| t.as_str()) {
        Some("notification") => Frame::Notification(
            value
                .get("seq")
                .and_then(serde_json::Value::as_i64)
                .filter(|n| *n >= 0),
        ),
        Some("notifications") => match count("unread") {
            Some(unread) => Frame::Notifications(ReadSync {
                unread,
                unread_for_you: count("unreadForYou"),
                read_ids: value
                    .get("readIds")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(serde_json::Value::as_str)
                            .take(MAX_READ_IDS)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                all: flag("all"),
                resync: flag("resync"),
            }),
            None => Frame::Other,
        },
        Some("resync") => match value.get("seq").and_then(serde_json::Value::as_i64) {
            Some(seq) => Frame::Resync {
                seq,
                complete: flag("complete"),
            },
            None => Frame::Other,
        },
        Some("session")
            if value.get("state").and_then(serde_json::Value::as_str) == Some("ended") =>
        {
            Frame::SessionEnded
        }
        Some("presence") => Winner::of(&value).map_or(Frame::Other, Frame::Presence),
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
    Read(Read),
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

/// `/api/notifications`, or its `?since=` read (I-19).
pub fn notifications_url(origin: &str, since: Option<i64>) -> String {
    match since {
        Some(seq) => format!("{origin}/api/notifications?since={seq}"),
        None => format!("{origin}/api/notifications"),
    }
}

/// `/api/me/events`, with `?since=` once the watcher has read through a seq,
/// so a reopened stream replays what the cut lost (I-19). An instance that
/// predates replay ignores the query.
pub fn events_url(origin: &str, since: Option<i64>) -> String {
    match since {
        Some(seq) => format!("{origin}/api/me/events?since={seq}"),
        None => format!("{origin}/api/me/events"),
    }
}

async fn fetch(client: &reqwest::Client, origin: &str, cookie: &str, since: Option<i64>) -> Fetch {
    let response = client
        .get(notifications_url(origin, since))
        .header(COOKIE, cookie)
        .header(ACCEPT, "application/json")
        .send()
        .await;
    match response {
        Ok(r) if signed_out(r.status(), r.headers()) => Fetch::SignedOut,
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => parse_read(&v).map_or(Fetch::Failed, Fetch::Read),
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

/// Where a watcher starts: its tracker, and what the instance said about
/// itself on the baseline read.
pub struct Start {
    pub arrivals: Arrivals,
    pub server: Server,
}

impl Start {
    /// The page's own record replaces the fresh baseline: its seq, when it
    /// has one, is where the watcher reads from; without one, the first read
    /// is a plain read under wave 1's rule, and its snapshot seq starts the
    /// cursor.
    pub fn with_handoff(self, handoff: &Handoff) -> Self {
        Self {
            arrivals: Arrivals::from_handoff(handoff),
            ..self
        }
    }
}

/// What the shell's first read of an instance's notifications found, taken
/// while its page is still open.
pub enum Baseline {
    /// The rows the page has already surfaced: the watcher starts from them.
    Ready(Start),
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
/// `BASELINE_WAIT`. With `since` served, the baseline's snapshot seq is the
/// cursor's start: the baseline surfaces nothing filed before it, by
/// definition, and every later row has a higher seq (I-10).
pub async fn baseline(origin: &str, cookie: &str) -> Baseline {
    let read = tokio::time::timeout(BASELINE_WAIT, fetch(&client(), origin, cookie, None)).await;
    match read.unwrap_or(Fetch::Failed) {
        Fetch::Read(read) => {
            let mut arrivals = Arrivals::default();
            arrivals.arrive(&read.rows);
            if read.server.has("since")
                && let Some(seq) = read.seq
            {
                arrivals.read_through(seq);
            }
            Baseline::Ready(Start {
                arrivals,
                server: read.server,
            })
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
    pub server: Server,
    pub shared: Shared,
    /// The presence client id the instance's page used on this desktop (its
    /// session's `talaria.notify.client`), which is this device too: the
    /// page's last heartbeat outlives its suspend in the store by up to 75 s.
    pub page_client: Option<String>,
}

/// Start watching. The handle aborts it.
pub fn spawn(watch: Watch) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(run(watch))
}

struct SignedOut;

/// The watcher's running state beside its `Watch`.
struct Running {
    client: reqwest::Client,
    /// The rows the summary banner on screen stands for: once every one of
    /// them is read elsewhere, it closes too.
    summary: HashSet<String>,
    /// The last heartbeat's state, so only a change is logged.
    beat: Arc<Mutex<Option<&'static str>>>,
    /// The person's presence winner as last heard (I-17): from a `presence`
    /// frame, a heartbeat's answer, or a read of `/api/me/presence` before
    /// the first post. None until one of them has said.
    winner: Arc<Mutex<Option<Winner>>>,
}

async fn run(mut watch: Watch) {
    let label = watch.label.clone();
    let mut state = Running {
        client: client(),
        summary: HashSet::new(),
        beat: Arc::new(Mutex::new(None)),
        winner: Arc::new(Mutex::new(None)),
    };
    eprintln!(
        "talaria-desktop: {label}: watching notifications{}{}",
        match watch.arrivals.seq() {
            Some(seq) => format!(" from seq {seq}"),
            None => String::new(),
        },
        match &watch.page_client {
            Some(id) => format!(" (its page was presence client {id})"),
            None => String::new(),
        }
    );
    let mut backoff = Backoff::default();
    // The first read is now: with the page's record, it posts what the page
    // missed. The first heartbeat is now too.
    let mut next_poll = Instant::now();
    let mut next_beat = Instant::now();
    loop {
        let stream = state
            .client
            .get(events_url(&watch.origin, watch.arrivals.seq()))
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
            // A `?since` read finds new rows, never rows read since: a cut
            // stream may have lost a read-sync frame, so a reopen also looks
            // at the plain list once, for banners whose rows are read now.
            let mut reopened = watch.server.has("since") && watch.arrivals.seq().is_some();
            loop {
                let read = tokio::time::timeout_at(next_poll.min(next_beat), r.chunk()).await;
                let mut check = false;
                let mut list = std::mem::take(&mut reopened);
                let mut ended = false;
                match read {
                    Ok(Ok(Some(bytes))) => {
                        for data in frames.push(&bytes) {
                            match frame(&data) {
                                Frame::Notification(seq) => {
                                    // A replayed or live frame at or below
                                    // the seq already read through is one
                                    // this watcher has judged.
                                    let through = watch.arrivals.seq();
                                    if seq.zip(through).is_none_or(|(s, t)| s > t) {
                                        check = true;
                                    }
                                }
                                Frame::Notifications(sync) => {
                                    read_sync(&watch, &mut state, &sync).await;
                                    list |= sync.resync;
                                }
                                Frame::Resync { seq, complete } => {
                                    if resync_needs_read(seq, complete, watch.arrivals.seq()) {
                                        check = true;
                                    }
                                }
                                Frame::SessionEnded => ended = true,
                                Frame::Presence(winner) => {
                                    *state.winner.lock().unwrap() = Some(winner);
                                }
                                Frame::Other => {}
                            }
                        }
                    }
                    // Ended or broke: reconnect with backoff.
                    Ok(_) => break,
                    // A timer.
                    Err(_) => {}
                }
                let now = Instant::now();
                if now >= next_beat {
                    next_beat = now + BEAT;
                    heartbeat(&watch, &state);
                }
                if now >= next_poll {
                    check = true;
                    next_poll = now + POLL;
                }
                if list && close_read_banners(&mut watch, &mut state).await.is_err() {
                    return;
                }
                if check && read_and_post(&mut watch, &mut state).await.is_err() {
                    return;
                }
                if ended {
                    eprintln!("talaria-desktop: {label}: the session behind the stream ended");
                    break;
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
        // The poll floor, and the heartbeat, keep running through the outage.
        loop {
            tokio::time::sleep_until(next_poll.min(next_beat).min(reconnect_at)).await;
            let now = Instant::now();
            if now >= next_beat {
                next_beat = now + BEAT;
                heartbeat(&watch, &state);
            }
            if now >= next_poll {
                next_poll = now + POLL;
                eprintln!("talaria-desktop: {label}: poll floor read while the stream is down");
                if read_and_post(&mut watch, &mut state).await.is_err() {
                    return;
                }
            }
            if now >= reconnect_at {
                break;
            }
        }
    }
}

/// One read of the rows — `?since` page by page where the instance serves it
/// and the watcher has a cursor, else a plain read — then the badge share,
/// then every arrival posted.
async fn read_and_post(watch: &mut Watch, state: &mut Running) -> Result<(), SignedOut> {
    let mut fresh = Vec::new();
    let mut last = None;
    let since_mode = watch.server.has("since") && watch.arrivals.seq().is_some();
    let mut rebased = false;
    if since_mode {
        let mut cursor = watch.arrivals.seq();
        let mut pages = SincePages::default();
        let mut behind = None;
        for _ in 0..MAX_SINCE_PAGES {
            match fetch(&state.client, &watch.origin, &watch.cookie, cursor).await {
                Fetch::Read(read) => {
                    behind = server_behind(cursor, &read);
                    if behind.is_none() {
                        fresh.extend(watch.arrivals.arrive(&read.rows));
                    }
                    let next = pages.page(&read);
                    watch.server = read.server.clone();
                    last = Some(read);
                    match next {
                        Some(end) => cursor = Some(end),
                        None => break,
                    }
                }
                Fetch::SignedOut => return Err(signed_out_now(watch)),
                Fetch::Failed => break,
            }
        }
        match behind {
            // A restore under the watcher: rebase on the newest rows.
            Some(server) => {
                eprintln!(
                    "talaria-desktop: {}: the server's seq {server} is below the cursor {}; rebasing on the newest rows",
                    watch.label,
                    watch.arrivals.seq().unwrap_or_default()
                );
                match fetch(&state.client, &watch.origin, &watch.cookie, None).await {
                    Fetch::Read(read) => {
                        let seq = read.seq.unwrap_or(server);
                        fresh.extend(watch.arrivals.rebase(&read.rows, seq));
                        watch.server = read.server.clone();
                        last = Some(read);
                        rebased = true;
                    }
                    Fetch::SignedOut => return Err(signed_out_now(watch)),
                    // The next read tries again from the old cursor.
                    Fetch::Failed => {}
                }
            }
            None => {
                if let Some(seq) = pages.through() {
                    watch.arrivals.read_through(seq);
                }
            }
        }
    } else {
        match fetch(&state.client, &watch.origin, &watch.cookie, None).await {
            Fetch::Read(read) => {
                fresh = watch.arrivals.arrive(&read.rows);
                // The first plain read against an instance that serves
                // `since` starts the cursor at its snapshot: every row at or
                // below it was in this read or older than its newest 50, which
                // wave 1's rule could not reach either.
                if read.server.has("since")
                    && watch.arrivals.seq().is_none()
                    && let Some(seq) = read.seq
                {
                    watch.arrivals.read_through(seq);
                }
                watch.server = read.server.clone();
                last = Some(read);
            }
            Fetch::SignedOut => return Err(signed_out_now(watch)),
            Fetch::Failed => {}
        }
    }
    if let Some(read) = &last {
        if let Some(badge) = read.badge {
            crate::notify::set_badge(&watch.app, &watch.label, badge);
        }
        // A plain read also says which banners stand for rows read since.
        if !since_mode || rebased {
            close_read_rows(watch, state, &read.rows).await;
        }
    }
    watch.shared.lock().unwrap().seq = watch.arrivals.seq();
    post_all(watch, state, fresh).await;
    Ok(())
}

fn signed_out_now(watch: &Watch) -> SignedOut {
    eprintln!(
        "talaria-desktop: {}: signed out; no notifications while suspended",
        watch.label
    );
    SignedOut
}

/// A `notifications` frame (I-16): the badge share, then this instance's own
/// banners for the rows read elsewhere.
async fn read_sync(watch: &Watch, state: &mut Running, sync: &ReadSync) {
    crate::notify::set_badge(&watch.app, &watch.label, sync.badge());
    if sync.all {
        let closed = crate::notify::close_label(&watch.app, &watch.label).await;
        state.summary.clear();
        if closed > 0 {
            eprintln!(
                "talaria-desktop: {}: everything was read elsewhere; closed {closed} banner(s)",
                watch.label
            );
        }
        return;
    }
    if sync.read_ids.is_empty() {
        return;
    }
    let mut tags = sync.read_ids.clone();
    if !state.summary.is_empty() {
        for id in &sync.read_ids {
            state.summary.remove(id);
        }
        if state.summary.is_empty() {
            tags.push(SUMMARY_TAG.to_string());
        }
    }
    let closed = crate::notify::close(&watch.app, &watch.label, &tags).await;
    if closed > 0 {
        eprintln!(
            "talaria-desktop: {}: {} row(s) read elsewhere; closed {closed} banner(s)",
            watch.label,
            sync.read_ids.len()
        );
    }
}

/// `resync` (I-16): too many rows were read to name. A plain read says which
/// of this instance's banners stand for a row that is read now.
async fn close_read_banners(watch: &mut Watch, state: &mut Running) -> Result<(), SignedOut> {
    match fetch(&state.client, &watch.origin, &watch.cookie, None).await {
        Fetch::Read(read) => {
            if let Some(badge) = read.badge {
                crate::notify::set_badge(&watch.app, &watch.label, badge);
            }
            close_read_rows(watch, state, &read.rows).await;
            Ok(())
        }
        Fetch::SignedOut => Err(signed_out_now(watch)),
        Fetch::Failed => Ok(()),
    }
}

/// Close this instance's banners for every row in `rows` that is read.
async fn close_read_rows(watch: &Watch, state: &mut Running, rows: &[Row]) {
    let read: Vec<String> = rows
        .iter()
        .filter(|r| !r.unread())
        .map(|r| r.id.clone())
        .collect();
    if read.is_empty() {
        return;
    }
    let mut tags = read.clone();
    if !state.summary.is_empty() {
        for id in &read {
            state.summary.remove(id);
        }
        if state.summary.is_empty() {
            tags.push(SUMMARY_TAG.to_string());
        }
    }
    let closed = crate::notify::close(&watch.app, &watch.label, &tags).await;
    if closed > 0 {
        eprintln!(
            "talaria-desktop: {}: closed {closed} banner(s) whose rows are read",
            watch.label
        );
    }
}

/// The instance id behind a webview label.
fn instance_id(label: &str) -> &str {
    label.strip_prefix("instance-").unwrap_or(label)
}

/// This watcher's presence client id (I-17): `desk-<instance id>`.
pub fn client_id(label: &str) -> String {
    format!("desk-{}", instance_id(label))
}

/// Where the page runtime keeps its per-tab presence client id.
pub const PAGE_CLIENT_KEY: &str = "talaria.notify.client";

/// The page's presence client id from its suspended session, when it is one
/// the store would take (`[A-Za-z0-9_-]{8,64}`). A restore seeds the session
/// back, so the restored page beats under the same id.
pub fn page_client(session: &[(String, String)]) -> Option<String> {
    session
        .iter()
        .find(|(k, _)| k == PAGE_CLIENT_KEY)
        .map(|(_, v)| v.trim_matches('"').to_string())
        .filter(|v| {
            (8..=64).contains(&v.len())
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// The client the person is on, when it is another one (see `Winner`). The
/// winner comes from the stream's `presence` frames and each heartbeat's
/// answer; before either has said (the first read after a suspend), it is
/// read once from `GET /api/me/presence`. An instance without the presence
/// store, or a read that fails, has no winner: every client keeps its own
/// rule, as in wave 1.
async fn elsewhere(watch: &Watch, state: &Running) -> Option<String> {
    if !watch.server.has("presence-store") {
        return None;
    }
    let known = state.winner.lock().unwrap().clone();
    let winner = match known {
        Some(w) => w,
        None => {
            let read = state
                .client
                .get(format!("{}/api/me/presence", watch.origin))
                .timeout(WRITE_TIMEOUT)
                .header(COOKIE, &watch.cookie)
                .header(ACCEPT, "application/json")
                .send()
                .await;
            let w = match read {
                Ok(r) if r.status().is_success() => r
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .as_ref()
                    .and_then(Winner::of),
                _ => None,
            }?;
            *state.winner.lock().unwrap() = Some(w.clone());
            w
        }
    };
    let own: Vec<String> = std::iter::once(client_id(&watch.label))
        .chain(watch.page_client.clone())
        .collect();
    winner.elsewhere(&own).map(str::to_string)
}

/// The handback stays the size the page's record can use: its newest
/// `MAX_SEEN` (the record keeps no more), so a long suspension of a busy
/// instance does not grow it without bound. With a seq handed back too, the
/// record's cursor covers the rest.
fn bound_posted(posted: &mut Vec<(String, String)>) {
    if posted.len() <= MAX_SEEN * 2 {
        return;
    }
    posted.sort_by_key(|(_, at)| std::cmp::Reverse(parse_iso_ms(at).unwrap_or(i64::MIN)));
    posted.truncate(MAX_SEEN);
}

/// The heartbeat's state from the OS's idle time (I-17): `active` while the
/// OS saw input in the last two minutes; `idle` when it did not, or cannot
/// say (no input means nobody to defer a push for).
pub fn presence_state(os_idle_ms: Option<u64>) -> &'static str {
    match os_idle_ms {
        Some(ms) if ms <= ACTIVE_MS => "active",
        _ => "idle",
    }
}

/// `PUT /api/me/presence`'s body for this instance's watcher.
pub fn presence_body(label: &str, os_idle_ms: Option<u64>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "clientId": client_id(label),
        "kind": "desktop",
        "state": presence_state(os_idle_ms),
    });
    if let Some(ms) = os_idle_ms {
        body["osIdleMs"] = serde_json::json!(ms);
    }
    body
}

/// One heartbeat, off the stream's task: a slow instance never holds a read.
/// Only to an instance that stores presence: an older one reads any PUT as
/// "online", and a tray left running all night would say so.
fn heartbeat(watch: &Watch, state: &Running) {
    if !watch.server.has("presence-store") {
        return;
    }
    let client = state.client.clone();
    let origin = watch.origin.clone();
    let cookie = watch.cookie.clone();
    let label = watch.label.clone();
    let last = state.beat.clone();
    let winner = state.winner.clone();
    tauri::async_runtime::spawn(async move {
        let idle = crate::notify::os_idle().await;
        let body = presence_body(&label, idle);
        let state = presence_state(idle);
        let sent = client
            .put(format!("{origin}/api/me/presence"))
            .timeout(WRITE_TIMEOUT)
            .header(COOKIE, &cookie)
            .header(ORIGIN, &origin)
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await;
        let outcome = match sent {
            Ok(r) if r.status().is_success() => {
                // The answer names the winner after this beat (I-17).
                if let Some(w) = r
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .as_ref()
                    .and_then(Winner::of)
                {
                    *winner.lock().unwrap() = Some(w);
                }
                Some(state)
            }
            Ok(r) => {
                eprintln!(
                    "talaria-desktop: {label}: the presence heartbeat answered {}",
                    r.status()
                );
                None
            }
            Err(e) => {
                eprintln!("talaria-desktop: {label}: the presence heartbeat: {e}");
                None
            }
        };
        let mut last = last.lock().unwrap();
        if outcome.is_some() && *last != outcome {
            eprintln!(
                "talaria-desktop: {label}: presence {state} (os idle {})",
                idle.map_or("unknown".to_string(), |ms| format!("{ms} ms"))
            );
        }
        *last = outcome;
    });
}

/// `POST /api/notifications/ack` (I-18) for rows this watcher just posted:
/// the person was told here, so the server's deferred push for them is
/// cancelled. Only to an instance that serves the ack.
async fn ack(watch: &Watch, state: &Running, ids: &[String]) {
    if ids.is_empty() || !watch.server.has("ack") {
        return;
    }
    for chunk in ids.chunks(ACK_CHUNK) {
        let sent = state
            .client
            .post(format!("{}/api/notifications/ack", watch.origin))
            .timeout(WRITE_TIMEOUT)
            .header(COOKIE, &watch.cookie)
            .header(ORIGIN, &watch.origin)
            .header(CONTENT_TYPE, "application/json")
            .body(serde_json::json!({ "ids": chunk }).to_string())
            .send()
            .await;
        match sent {
            Ok(r) if r.status().is_success() => {
                let acked = r
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|v| v.get("acked").and_then(serde_json::Value::as_u64));
                eprintln!(
                    "talaria-desktop: {}: acked {} of {} posted",
                    watch.label,
                    acked.map_or("?".to_string(), |n| n.to_string()),
                    chunk.len()
                );
            }
            Ok(r) => eprintln!(
                "talaria-desktop: {}: the ack answered {}",
                watch.label,
                r.status()
            ),
            Err(e) => eprintln!("talaria-desktop: {}: the ack: {e}", watch.label),
        }
    }
}

/// The summary banner for a burst: "N new in <label>".
pub fn summary_title(n: usize, instance: &str) -> String {
    format!("{n} new in {instance}")
}

/// Split a read's arrivals into what may be bannered now and what is only
/// recorded: feed rows, and rows Do Not Disturb holds (I-15).
pub fn bannerable(server: &Server, fresh: Vec<Arrival>) -> (Vec<Arrival>, usize, usize) {
    let mut show = Vec::new();
    let (mut feed, mut held) = (0, 0);
    for arrival in fresh {
        if arrival.row.feed() {
            feed += 1;
        } else if !server.bannerable(&arrival.row) {
            held += 1;
        } else {
            show.push(arrival);
        }
    }
    (show, feed, held)
}

async fn post_all(watch: &Watch, state: &mut Running, fresh: Vec<Arrival>) {
    if fresh.is_empty() {
        return;
    }
    // Every arrival goes back to the page's record, posted or not: the page
    // would not banner a feed row, a held one or a muted instance's either.
    {
        let mut shared = watch.shared.lock().unwrap();
        for arrival in &fresh {
            shared
                .posted
                .push((arrival.row.id.clone(), arrival.row.created_text()));
        }
        bound_posted(&mut shared.posted);
    }
    let total = fresh.len();
    let (show, feed, held) = bannerable(&watch.server, fresh);
    if feed > 0 {
        eprintln!(
            "talaria-desktop: {}: {feed} feed row(s) recorded, not posted",
            watch.label
        );
    }
    if held > 0 {
        eprintln!(
            "talaria-desktop: {}: Do Not Disturb holds {held} of {total} arrival(s)",
            watch.label
        );
    }
    if show.is_empty() {
        return;
    }
    if crate::commands::is_muted(&watch.app, &watch.label) {
        eprintln!(
            "talaria-desktop: {}: muted; {} arrival(s) not posted",
            watch.label,
            show.len()
        );
        return;
    }
    // Cross-device (I-17, the page runtime's rule): the person is looking
    // at, or active on, another of their clients — a browser tab, another
    // desktop — so that one alerts and acks, and this one stays quiet.
    if let Some(other) = elsewhere(watch, state).await {
        eprintln!(
            "talaria-desktop: {}: the person is on another client ({other}); {} arrival(s) left to it",
            watch.label,
            show.len()
        );
        return;
    }
    if show.len() > SUMMARY_OVER {
        eprintln!(
            "talaria-desktop: {}: posting a summary of {} while suspended",
            watch.label,
            show.len()
        );
        let newest: Vec<String> = show
            .iter()
            .rev()
            .take(SUMMARY_OVER)
            .map(|a| crate::notify::clamped_title(&a.row.title))
            .filter(|t| !t.is_empty())
            .collect();
        let post = crate::notify::Post {
            label: watch.label.clone(),
            instance: watch.instance.clone(),
            title: summary_title(show.len(), &watch.instance),
            body: Some(crate::notify::clamped_body(&newest.join(" · "))).filter(|b| !b.is_empty()),
            tag: SUMMARY_TAG.into(),
            href: None,
            kind: None,
            renotify: true,
            source: crate::notify::Source::Summary,
        };
        let ids: Vec<String> = show.iter().map(|a| a.row.id.clone()).collect();
        match crate::notify::post(&watch.app, post).await {
            Ok(()) => {
                state.summary.extend(ids.iter().cloned());
                ack(watch, state, &ids).await;
            }
            Err(e) => eprintln!("talaria-desktop: {}: {e}", watch.label),
        }
        return;
    }
    for arrival in show {
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
        match crate::notify::post(&watch.app, post).await {
            Ok(()) => ack(watch, state, std::slice::from_ref(&row.id)).await,
            Err(e) => eprintln!("talaria-desktop: {}: {e}", watch.label),
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
            seq: None,
            alert: None,
            class: None,
        }
    }

    fn seq_row(id: &str, ms: i64, seq: i64) -> Row {
        Row {
            seq: Some(seq),
            ..row_at(id, ms)
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
    fn with_a_seq_the_record_decides_and_no_margin_applies() {
        let mut a = Arrivals::default();
        a.arrive(&[seq_row("1", T0, 10)]);
        a.read_through(10);
        // Past the seq: an arrival however old its createdAt (a row whose
        // Quiet coalesce kept an old createdAt, or a skewed clock).
        let old_but_new = seq_row("2", T0 - 10 * MARGIN_MS, 11);
        // At or below the seq: never, even though it is unseen and recent.
        let judged = seq_row("3", T0 + 5_000, 9);
        assert_eq!(ids(&a.arrive(&[judged, old_but_new])), vec!["2"]);
        // A legacy row (no seq) beside them keeps wave 1's rule.
        let legacy_new = row_at("4", T0 + 1_000);
        let legacy_old = row_at("5", T0 - MARGIN_MS - 1);
        assert_eq!(ids(&a.arrive(&[legacy_new, legacy_old])), vec!["4"]);
        // A re-alert past the seq: the same id, a later createdAt.
        let fresh = a.arrive(&[seq_row("2", T0 + 90_000, 12)]);
        assert_eq!(ids(&fresh), vec!["2"]);
        assert!(fresh[0].realert);
        // A Quiet coalesce: a new seq, the same createdAt — not an alert.
        assert!(a.arrive(&[seq_row("2", T0 + 90_000, 13)]).is_empty());
        // The seq only moves forward.
        a.read_through(5);
        assert_eq!(a.seq(), Some(10));
        a.read_through(13);
        assert_eq!(a.seq(), Some(13));
    }

    #[test]
    fn a_since_page_in_seq_order_comes_out_oldest_first_and_once() {
        let mut a = Arrivals::default();
        a.arrive(&[]);
        a.read_through(0);
        let page: Vec<Row> = (1..=60)
            .map(|i| seq_row(&format!("r{i}"), T0 + i * 1_000, i))
            .collect();
        let fresh = a.arrive(&page);
        assert_eq!(fresh.len(), 60);
        assert_eq!(fresh.first().unwrap().row.id, "r1");
        assert_eq!(fresh.last().unwrap().row.id, "r60");
        a.read_through(60);
        // The same page again (a replay racing the read): nothing.
        assert!(a.arrive(&page).is_empty());
        // A duplicate inside one read is one arrival.
        let dup = vec![seq_row("x", T0 + 61_000, 61), seq_row("x", T0 + 61_000, 61)];
        assert_eq!(ids(&a.arrive(&dup)), vec!["x"]);
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
            seq: None,
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
        // A v2 record's seq is where the watcher reads from.
        let v2 = Handoff {
            seq: Some(41),
            ..handoff
        };
        let mut b = Arrivals::from_handoff(&v2);
        assert_eq!(b.seq(), Some(41));
        assert_eq!(
            ids(&b.arrive(&[seq_row("42", T0 - 600_000, 42), seq_row("40", T0 + 1, 40)])),
            vec!["42"]
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
            seq: None,
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
            "at": "x",
            "seq": "12"
        }))
        .unwrap();
        assert_eq!(lenient.seen.len(), 1);
        assert_eq!((lenient.hw, lenient.at, lenient.seq), (None, None, None));
        let v2 =
            Handoff::from_value(&serde_json::json!({"user": "u1", "v": 2, "seq": 12})).unwrap();
        assert_eq!(v2.seq, Some(12));
        assert_eq!(
            Handoff::from_value(&serde_json::json!({"user": "u1", "seq": -3}))
                .unwrap()
                .seq,
            None
        );
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
        // The replay's framing (I-19): an `id:` line before the data.
        assert_eq!(
            f.push(b"id: 42\ndata: {\"type\":\"notification\",\"seq\":42}\n\n"),
            vec![r#"{"type":"notification","seq":42}"#.to_string()]
        );
        assert_eq!(
            frame(r#"{"type":"notification","notificationId":"n1"}"#),
            Frame::Notification(None)
        );
        assert_eq!(
            frame(r#"{"type":"notification","notificationId":"n1","seq":7}"#),
            Frame::Notification(Some(7))
        );
        assert_eq!(
            frame(r#"{"type":"notifications","unread":4}"#),
            Frame::Notifications(ReadSync {
                unread: 4,
                ..ReadSync::default()
            })
        );
        assert_eq!(frame(r#"{"type":"notifications"}"#), Frame::Other);
        assert_eq!(frame(r#"{"type":"run","runId":"r"}"#), Frame::Other);
        assert_eq!(frame("not json"), Frame::Other);
    }

    #[test]
    fn the_contract_frames_parse_and_an_older_shape_still_does() {
        let sync = frame(
            r#"{"type":"notifications","unread":5,"unreadForYou":2,"readIds":["a","b",3],"all":false}"#,
        );
        assert_eq!(
            sync,
            Frame::Notifications(ReadSync {
                unread: 5,
                unread_for_you: Some(2),
                read_ids: vec!["a".into(), "b".into()],
                all: false,
                resync: false,
            })
        );
        let Frame::Notifications(sync) = sync else {
            unreachable!()
        };
        // The badge is For you; an older api's frame is every unread row.
        assert_eq!(sync.badge(), 2);
        let Frame::Notifications(old) = frame(r#"{"type":"notifications","unread":5}"#) else {
            unreachable!()
        };
        assert_eq!(old.badge(), 5);
        let Frame::Notifications(all) = frame(r#"{"type":"notifications","unread":0,"all":true}"#)
        else {
            unreachable!()
        };
        assert!(all.all && all.read_ids.is_empty());
        let Frame::Notifications(many) =
            frame(r#"{"type":"notifications","unread":0,"resync":true}"#)
        else {
            unreachable!()
        };
        assert!(many.resync);
        let hundreds: Vec<String> = (0..150).map(|i| format!("\"r{i}\"")).collect();
        let Frame::Notifications(capped) = frame(&format!(
            r#"{{"type":"notifications","unread":1,"readIds":[{}]}}"#,
            hundreds.join(",")
        )) else {
            unreachable!()
        };
        assert_eq!(capped.read_ids.len(), MAX_READ_IDS);
        assert_eq!(
            frame(r#"{"type":"resync","seq":9,"complete":true}"#),
            Frame::Resync {
                seq: 9,
                complete: true
            }
        );
        assert_eq!(
            frame(r#"{"type":"resync","seq":9}"#),
            Frame::Resync {
                seq: 9,
                complete: false
            }
        );
        assert_eq!(frame(r#"{"type":"resync"}"#), Frame::Other);
        assert_eq!(
            frame(r#"{"type":"session","state":"ended"}"#),
            Frame::SessionEnded
        );
        assert_eq!(frame(r#"{"type":"session","state":"x"}"#), Frame::Other);
        assert_eq!(
            frame(r#"{"type":"presence","looking":null,"active":"desk-a","clients":1}"#),
            Frame::Presence(Winner {
                looking: None,
                active: Some("desk-a".into()),
            })
        );
        assert_eq!(frame(r#"{"type":"presence","clients":0}"#), Frame::Other);
    }

    #[test]
    fn the_watcher_stays_quiet_while_the_person_is_on_another_client() {
        assert_eq!(client_id("instance-aaaa"), "desk-aaaa");
        // This desktop: the watcher, and the instance's own page here.
        let own = vec![client_id("instance-aaaa"), "page-tab-aaaa".to_string()];
        let looking_web =
            Winner::of(&serde_json::json!({"ok": true, "looking": "tab-1", "active": null}))
                .unwrap();
        assert_eq!(looking_web.elsewhere(&own), Some("tab-1"));
        // Active on another desktop, nobody looking: that one alerts.
        let other_desk =
            Winner::of(&serde_json::json!({"looking": null, "active": "desk-bbbb"})).unwrap();
        assert_eq!(other_desk.elsewhere(&own), Some("desk-bbbb"));
        // This watcher wins, or nobody does (idle everywhere): it alerts.
        let me = Winner::of(&serde_json::json!({"looking": null, "active": "desk-aaaa"})).unwrap();
        assert_eq!(me.elsewhere(&own), None);
        let nobody = Winner::of(&serde_json::json!({"looking": null, "active": null})).unwrap();
        assert_eq!(nobody.elsewhere(&own), None);
        // The instance's own page, suspended a moment ago: its last beat is
        // still the newest active one, and it is this desktop, not another.
        let my_page =
            Winner::of(&serde_json::json!({"looking": null, "active": "page-tab-aaaa"})).unwrap();
        assert_eq!(my_page.elsewhere(&own), None);
        // Looking beats active, as the server's rule has it.
        let both =
            Winner::of(&serde_json::json!({"looking": "tab-1", "active": "desk-aaaa"})).unwrap();
        assert_eq!(both.elsewhere(&own), Some("tab-1"));
        // A wave-1 ping's answer names no winner at all.
        assert_eq!(Winner::of(&serde_json::json!({"ok": true})), None);
        assert_eq!(
            Winner::of(&serde_json::json!({"looking": "", "active": 7})),
            Some(Winner::default())
        );
    }

    #[test]
    fn the_page_client_comes_from_the_suspended_session() {
        let s = |v: &str| {
            vec![
                ("x".to_string(), "y".to_string()),
                (PAGE_CLIENT_KEY.to_string(), v.to_string()),
            ]
        };
        assert_eq!(
            page_client(&s("tV3bZZd6ew2ztx0ui1ijwxA")).as_deref(),
            Some("tV3bZZd6ew2ztx0ui1ijwxA")
        );
        assert_eq!(
            page_client(&s("\"abcdefgh12\"")).as_deref(),
            Some("abcdefgh12")
        );
        assert_eq!(page_client(&s("short")), None);
        assert_eq!(page_client(&s("bad id!!!")), None);
        assert_eq!(page_client(&[]), None);
    }

    fn since_page(seqs: &[i64], more: bool, snapshot: i64) -> Read {
        Read {
            rows: seqs
                .iter()
                .map(|s| seq_row(&format!("r{s}"), T0 + s, *s))
                .collect(),
            badge: None,
            seq: Some(snapshot),
            more,
            server: Server::default(),
        }
    }

    #[test]
    fn a_catch_up_past_the_page_budget_reads_on_from_its_last_row() {
        // The last page: the snapshot seq is the read-through.
        let mut done = SincePages::default();
        assert_eq!(done.page(&since_page(&[11, 12], false, 14)), None);
        assert_eq!(done.through(), Some(14));
        // Pages with more: each moves the cursor to its last row, and a
        // catch-up cut short (the budget, a failed page) keeps that.
        let mut cut = SincePages::default();
        assert_eq!(cut.page(&since_page(&[11, 12, 13], true, 900)), Some(13));
        assert_eq!(cut.page(&since_page(&[14, 15], true, 900)), Some(15));
        assert_eq!(
            cut.through(),
            Some(15),
            "never the snapshot of a page with more"
        );
        // `more` with no row to go on from ends it, and moves nothing.
        let mut odd = SincePages::default();
        assert_eq!(odd.page(&since_page(&[], true, 900)), None);
        assert_eq!(odd.through(), None);
        // Why: the seen set keeps 300, so a catch-up that started over from
        // its old cursor would post again every row the set let go of.
        let rows: Vec<Row> = (11..=1_200)
            .map(|s| seq_row(&format!("r{s}"), T0 + s, s))
            .collect();
        let mut over = Arrivals::default();
        over.arrive(&[]);
        over.read_through(10);
        assert_eq!(over.arrive(&rows[..1_000]).len(), 1_000);
        assert_eq!(
            over.arrive(&rows[..1_000]).len(),
            700,
            "the old cursor posts again what the seen set let go of"
        );
        let mut on = Arrivals::default();
        on.arrive(&[]);
        on.read_through(10);
        on.arrive(&rows[..1_000]);
        on.read_through(1_010);
        assert_eq!(
            on.arrive(&rows).len(),
            190,
            "read on from the last row: only the rest"
        );
    }

    #[test]
    fn a_server_seq_below_the_cursor_is_a_restore_and_the_watcher_rebases() {
        // Only a complete page whose snapshot is below the cursor says so.
        assert_eq!(
            server_behind(Some(100), &since_page(&[], false, 95)),
            Some(95)
        );
        assert_eq!(server_behind(Some(100), &since_page(&[], false, 100)), None);
        assert_eq!(
            server_behind(Some(100), &since_page(&[101], false, 101)),
            None
        );
        assert_eq!(server_behind(Some(100), &since_page(&[], true, 95)), None);
        assert_eq!(server_behind(None, &since_page(&[], false, 95)), None);
        // The watcher read through 100; the restore put the server back at
        // 95, and two rows were filed after it: seqs 96 and 97, both at or
        // below the old cursor.
        let mut a = Arrivals::default();
        a.arrive(&[seq_row("old", T0, 100)]);
        a.read_through(100);
        let since_restore = [
            seq_row("n2", T0 + 60_000, 97),
            seq_row("n1", T0 + 30_000, 96),
        ];
        // The defect: by the record's seq, neither is ever news.
        assert!(a.clone().arrive(&since_restore).is_empty());
        // Rebased on the newest page (newest first, as a plain read is): the
        // rows filed since the restore surface, oldest first; a row from
        // before it (older than the high-water mark's margin) does not; the
        // cursor is the server's.
        let before = seq_row("pre", T0 - MARGIN_MS - 1, 90);
        let mut page = since_restore.to_vec();
        page.push(before);
        assert_eq!(ids(&a.rebase(&page, 97)), vec!["n1", "n2"]);
        assert_eq!(a.seq(), Some(97));
        // Then the record rule again, from the server's seq.
        assert_eq!(
            ids(&a.arrive(&[seq_row("n3", T0 + 90_000, 98)])),
            vec!["n3"]
        );
        assert!(a.arrive(&page).is_empty());
    }

    #[test]
    fn a_resync_below_the_cursor_asks_for_a_read() {
        assert!(!resync_needs_read(50, true, Some(50)), "nothing new");
        assert!(
            resync_needs_read(51, true, Some(50)),
            "rows past the cursor"
        );
        assert!(
            resync_needs_read(40, true, Some(50)),
            "a restore: the read rebases"
        );
        assert!(
            resync_needs_read(50, false, Some(50)),
            "an incomplete replay"
        );
        assert!(resync_needs_read(50, true, None), "no cursor yet");
    }

    #[test]
    fn the_handback_keeps_the_newest_the_record_can_use() {
        let mut posted: Vec<(String, String)> = (0..(MAX_SEEN * 2) as i64)
            .map(|i| (format!("n{i}"), iso_of_ms(T0 + i)))
            .collect();
        bound_posted(&mut posted);
        assert_eq!(posted.len(), MAX_SEEN * 2, "within the bound: untouched");
        posted.push(("newest".into(), iso_of_ms(T0 + 10_000)));
        bound_posted(&mut posted);
        assert_eq!(posted.len(), MAX_SEEN);
        assert_eq!(posted[0].0, "newest");
        assert!(!posted.iter().any(|(id, _)| id == "n0"));
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
        let p = parse_read(&serde_json::from_str(
            r#"{"notifications":[{"id":"n1","kind":"dm","title":"t","body":"b","href":"/c","readAt":null,"createdAt":"2026-10-31T10:00:00.000Z"}],"unread":3,"prompt":{"x":1}}"#,
        ).unwrap())
        .unwrap();
        assert_eq!(p.badge, Some(3));
        assert!(p.rows[0].unread());
        assert_eq!(p.rows[0].kind, "dm");
        assert_eq!((p.rows[0].seq, p.seq, p.more), (None, None, false));
        assert!(p.server.features.is_empty());
        let older = parse_read(&serde_json::json!({"notifications":[{"id":"n1"}]})).unwrap();
        assert_eq!(older.badge, None);
        assert_eq!(older.rows[0].created_ms(), None);
        // A row that does not parse is skipped, not the whole read.
        let mixed =
            parse_read(&serde_json::json!({"notifications":[{"id":"n1"},{"nope":1},7]})).unwrap();
        assert_eq!(mixed.rows.len(), 1);
        assert!(parse_read(&serde_json::json!([1, 2])).is_none());
    }

    #[test]
    fn the_contract_payload_gives_seq_for_you_features_and_dnd() {
        // Captured from the contract's api (slot 6), trimmed.
        let p = parse_read(&serde_json::json!({
            "notifications": [
                {"id":"n2","kind":"approval_pending","title":"Approve","body":"","href":"/a",
                 "readAt":null,"createdAt":"2026-11-01T01:00:00.000Z","seq":8,"alert":"banner",
                 "class":"approval_pending","category":"needs_you","count":1},
                {"id":"n1","kind":"task-status","title":"Moved","body":"","href":"/b",
                 "readAt":null,"createdAt":"2026-11-01T00:59:00.000Z","seq":7,"alert":"feed",
                 "class":"work_complete","category":"following","count":1}
            ],
            "unread": 5, "unreadForYou": 2, "seq": 8, "more": false,
            "features": ["seq","classes","ack","presence-store","since"],
            "dnd": {"pausedUntil":"2026-11-01T02:00:00.000Z","schedule":null,"allowNeedsYou":true,
                    "active":true,"until":"2026-11-01T02:00:00.000Z","reason":"paused","timeZone":"UTC"},
            "classes": [
                {"id":"approval_pending","breaksThrough":true},
                {"id":"dm","breaksThrough":false},
                {"id":"work_complete"}
            ]
        }))
        .unwrap();
        assert_eq!(p.badge, Some(2), "the badge is For you");
        assert_eq!(p.seq, Some(8));
        assert!(p.server.has("since") && p.server.has("ack"));
        assert!(p.server.dnd_active && p.server.allow_needs_you);
        assert_eq!(
            p.server.breaks,
            ["approval_pending".to_string()].into_iter().collect()
        );
        assert_eq!(p.rows[0].seq, Some(8));
        assert!(p.rows[1].feed());
        // Do Not Disturb: only the break-through class; never a feed row.
        let fresh: Vec<Arrival> = p
            .rows
            .iter()
            .cloned()
            .map(|row| Arrival {
                row,
                realert: false,
            })
            .collect();
        let (show, feed, held) = bannerable(&p.server, fresh.clone());
        assert_eq!((ids(&show), feed, held), (vec!["n2"], 1, 0));
        let mut dm = fresh[0].clone();
        dm.row.class = Some("dm".into());
        dm.row.id = "n3".into();
        let (show, _, held) = bannerable(&p.server, vec![dm.clone()]);
        assert!(show.is_empty() && held == 1);
        // allowNeedsYou off: nothing breaks through.
        let strict = Server {
            allow_needs_you: false,
            ..p.server.clone()
        };
        assert!(bannerable(&strict, fresh.clone()).0.is_empty());
        // Do Not Disturb off: every banner row, still never a feed row.
        let off = Server {
            dnd_active: false,
            ..p.server.clone()
        };
        let (show, feed, _) = bannerable(&off, vec![dm, fresh[1].clone()]);
        assert_eq!((ids(&show), feed), (vec!["n3"], 1));
        // A `since` page that is not the last.
        let page = parse_read(&serde_json::json!({"notifications": [], "more": true, "seq": 300}))
            .unwrap();
        assert!(page.more);
    }

    #[test]
    fn the_urls_carry_since_only_once_there_is_a_cursor() {
        assert_eq!(
            notifications_url("https://a.example", None),
            "https://a.example/api/notifications"
        );
        assert_eq!(
            notifications_url("https://a.example", Some(41)),
            "https://a.example/api/notifications?since=41"
        );
        assert_eq!(
            events_url("https://a.example", Some(0)),
            "https://a.example/api/me/events?since=0"
        );
        assert_eq!(
            events_url("https://a.example", None),
            "https://a.example/api/me/events"
        );
    }

    #[test]
    fn the_heartbeat_is_active_within_two_minutes_of_input() {
        assert_eq!(presence_state(Some(0)), "active");
        assert_eq!(presence_state(Some(ACTIVE_MS)), "active");
        assert_eq!(presence_state(Some(ACTIVE_MS + 1)), "idle");
        // An OS that cannot say is nobody to defer a push for.
        assert_eq!(presence_state(None), "idle");
        let body = presence_body("instance-aaaa-1", Some(1_500));
        assert_eq!(
            body,
            serde_json::json!({"clientId":"desk-aaaa-1","kind":"desktop","state":"active","osIdleMs":1500})
        );
        let unknown = presence_body("instance-b", None);
        assert!(unknown.get("osIdleMs").is_none());
        // The contract's client id shape: [A-Za-z0-9_-]{8,64}.
        let id = presence_body("instance-0f5c2a3e-8b1d-4c6e-9a7b-1d2e3f4a5b6c", None)["clientId"]
            .as_str()
            .unwrap()
            .to_string();
        assert!((8..=64).contains(&id.len()));
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        );
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
    fn a_burst_is_one_summary_under_the_shared_tag() {
        assert_eq!(summary_title(5, "Acme"), "5 new in Acme");
        assert_eq!(SUMMARY_TAG, "talaria-summary");
    }
}
