// The per-user notification settings in `users.notify_prefs`: the class ×
// channel matrix (prefs v2, I-13) and the legacy routing table it stays in
// step with, the digest's derived answer, DND and working hours (I-14), the
// missed-activity mail frequency, the soft ask's memory under the reserved
// `prompt` key, and the instance email master switch in app_settings.
//
// Reserved top-level keys of the blob: the original class ids (legacy routes),
// `digest`, `prompt`, `channels`, `dnd`, `email`. Every reader DERIVES from
// whatever is stored, so a hand-edited row degrades to the defaults.

use serde_json::{Map, Value, json};
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_settings::{get_setting, set_setting};

use crate::classes::{
    CLASSES, ChannelDefaults, FALLBACK_CLASS, NOTIFY_CLASSES, ORIGINAL_CLASSES, class_def,
};

const DIGEST_PREF_KEY: &str = "digest";
const CHANNELS_KEY: &str = "channels";
const DND_KEY: &str = "dnd";
const EMAIL_KEY: &str = "email";
pub(crate) const DELIVERY_KEY: &str = "notify_delivery";
const ROUTES: &[&str] = &["in_app", "email", "both"];

fn is_route(v: &Value) -> bool {
    v.as_str().is_some_and(|s| ROUTES.contains(&s))
}

// ── Channels (prefs v2) ──────────────────────────────────────────────────────

/// One class's delivery channels as the wire serves them. `in_app` is always
/// true: the row is the record, and no setting can stop it landing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channels {
    pub in_app: bool,
    pub desktop: bool,
    pub push: bool,
    pub email: bool,
}

/// What a legacy route meant, in channels: wave 1 pushed whenever the route
/// kept an in-app copy and mailed whenever it was not `in_app`.
fn route_channels(route: &str) -> Option<ChannelDefaults> {
    Some(match route {
        "in_app" => ChannelDefaults {
            desktop: true,
            push: true,
            email: false,
        },
        "email" => ChannelDefaults {
            desktop: false,
            push: false,
            email: true,
        },
        "both" => ChannelDefaults {
            desktop: true,
            push: true,
            email: true,
        },
        _ => return None,
    })
}

/// The legacy route a set of channels reads as — what a rolled-back binary or
/// a stale bundle sees: email and a screen → `both`, email alone → `email`,
/// no email → `in_app`.
pub fn legacy_route(c: Channels) -> &'static str {
    match (c.email, c.desktop || c.push) {
        (true, true) => "both",
        (true, false) => "email",
        (false, _) => "in_app",
    }
}

/// One class's effective channels. Each bool resolves on its own:
/// 1. `channels.<class>.<channel>` when stored as a bool;
/// 2. else what the legacy route for the class meant;
/// 3. else the registry default (an unknown class: `work_complete`'s).
pub fn channels_of(blob: &Value, class: &str) -> Channels {
    let defaults = class_def(class)
        .or_else(|| class_def(FALLBACK_CLASS))
        .map(|c| c.defaults)
        .unwrap_or(ChannelDefaults {
            desktop: true,
            push: true,
            email: false,
        });
    let legacy = blob
        .get(class)
        .and_then(Value::as_str)
        .and_then(route_channels);
    let explicit = blob
        .get(CHANNELS_KEY)
        .and_then(|c| c.get(class))
        .and_then(Value::as_object);
    let pick = |key: &str, from: fn(&ChannelDefaults) -> bool| -> bool {
        explicit
            .and_then(|o| o.get(key))
            .and_then(Value::as_bool)
            .unwrap_or_else(|| from(legacy.as_ref().unwrap_or(&defaults)))
    };
    Channels {
        in_app: true,
        desktop: pick("desktop", |d| d.desktop),
        push: pick("push", |d| d.push),
        email: pick("email", |d| d.email),
    }
}

/// The whole matrix as the wire serves it (`channels`): class → channels, in
/// registry order. A member is not offered an admin-only class.
pub fn channels_wire(blob: &Value, is_admin: bool) -> Value {
    let mut m = Map::new();
    for c in CLASSES.iter().filter(|c| is_admin || !c.admin_only) {
        m.insert(
            c.id.to_string(),
            serde_json::to_value(channels_of(blob, c.id)).expect("plain bools serialize"),
        );
    }
    Value::Object(m)
}

/// The full legacy routing table, derived from the channels so the two can
/// never disagree: for a blob with no `channels` this is exactly wave 1's
/// answer (the stored route when it is one we understand, the class fallback
/// otherwise). Key order is NOTIFY_CLASSES order.
pub fn resolve_prefs(stored: &Value) -> Value {
    let mut m = Map::new();
    for (id, _) in NOTIFY_CLASSES {
        m.insert(
            (*id).to_string(),
            Value::String(legacy_route(channels_of(stored, id)).to_string()),
        );
    }
    Value::Object(m)
}

/// The effective digest answer: an explicit on/off wins outright; with no
/// explicit choice it is DERIVED in the direction that cannot spam — someone
/// who turned email off for every class is not mailed a digest either.
///
/// FROZEN TO THE ORIGINAL NINE (C27): the derivation reads `ORIGINAL_CLASSES`,
/// not the registry, so the first new class with email on by default cannot
/// switch a person's digest on by existing.
pub fn digest_enabled(stored: &Value) -> bool {
    if let Some(explicit) = stored.get(DIGEST_PREF_KEY).and_then(|v| v.as_str())
        && matches!(explicit, "on" | "off")
    {
        return explicit == "on";
    }
    ORIGINAL_CLASSES
        .iter()
        .any(|id| channels_of(stored, id).email)
}

// ── Writes, both ways ────────────────────────────────────────────────────────

/// A v2 change to one class's channels; `None` leaves a channel as stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelsPatch {
    pub desktop: Option<bool>,
    pub push: Option<bool>,
    pub email: Option<bool>,
}

/// Working hours: inside them notifications alert, outside they are quiet.
/// `days` are weekday numbers, 0 = Sunday (JavaScript's `getDay`). `start >
/// end` crosses midnight; `start == end` is the whole day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DndSchedule {
    pub enabled: bool,
    pub days: Vec<u8>,
    pub start: String,
    pub end: String,
}

/// A change to DND. The outer Option is "present"; `Some(None)` clears.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DndPatch {
    pub paused_until: Option<Option<String>>,
    pub schedule: Option<Option<DndSchedule>>,
    pub allow_needs_you: Option<bool>,
}

/// Everything a PATCH may change, each part optional.
#[derive(Clone, Debug, Default)]
pub struct PrefsWrite {
    /// Legacy routes, class → route (a stale bundle's write).
    pub prefs: Option<Map<String, Value>>,
    pub digest: Option<String>,
    /// v2 channels, class → patch, in the order sent.
    pub channels: Option<Vec<(String, ChannelsPatch)>>,
    pub dnd: Option<DndPatch>,
    pub email_frequency: Option<String>,
}

fn obj_mut<'a>(m: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let slot = m
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !slot.is_object() {
        *slot = Value::Object(Map::new());
    }
    slot.as_object_mut().expect("just made an object")
}

/// The blob after `w`, from the blob before it (any shape — a non-object is
/// the untouched case). Pure, so both directions are pinned by tests.
///
/// - A LEGACY write (`prefs`, a stale bundle) writes the route and ALSO the
///   class's channels, so a bundle that predates channels is still obeyed once
///   they exist: `email` → email on, desktop and push off; `in_app` / `both` →
///   email off / on, and desktop and push keep what was stored (true when
///   nothing was, and true again if both were off — those routes mean "show it
///   on my screens").
/// - A v2 write (`channels`) merges the bools it carries, then rewrites the
///   legacy route from the result, so a rolled-back binary that reads only the
///   routes behaves as close as it can.
///
/// Unknown classes and unknown keys are dropped, never stored.
pub fn merge_prefs(before: &Value, w: &PrefsWrite) -> Value {
    let mut m = before.as_object().cloned().unwrap_or_default();
    if let Some(prefs) = &w.prefs {
        for (id, _) in NOTIFY_CLASSES {
            let Some(route) = prefs
                .get(*id)
                .filter(|v| is_route(v))
                .and_then(Value::as_str)
            else {
                continue;
            };
            m.insert((*id).to_string(), Value::String(route.to_string()));
            let ch = obj_mut(obj_mut(&mut m, CHANNELS_KEY), id);
            ch.insert("email".into(), Value::Bool(route != "in_app"));
            if route == "email" {
                ch.insert("desktop".into(), Value::Bool(false));
                ch.insert("push".into(), Value::Bool(false));
            } else {
                let stored = |k: &str| ch.get(k).and_then(Value::as_bool);
                let (desktop, push) = match (stored("desktop"), stored("push")) {
                    (Some(false), Some(false)) => (true, true),
                    (d, p) => (d.unwrap_or(true), p.unwrap_or(true)),
                };
                ch.insert("desktop".into(), Value::Bool(desktop));
                ch.insert("push".into(), Value::Bool(push));
            }
        }
    }
    if let Some(channels) = &w.channels {
        for (id, patch) in channels {
            if class_def(id).is_none() {
                continue;
            }
            {
                let ch = obj_mut(obj_mut(&mut m, CHANNELS_KEY), id);
                for (k, v) in [
                    ("desktop", patch.desktop),
                    ("push", patch.push),
                    ("email", patch.email),
                ] {
                    if let Some(v) = v {
                        ch.insert(k.into(), Value::Bool(v));
                    }
                }
            }
            if NOTIFY_CLASSES.iter().any(|(c, _)| c == id) {
                let resolved = channels_of(&Value::Object(m.clone()), id);
                m.insert(id.clone(), Value::String(legacy_route(resolved).into()));
            }
        }
    }
    if let Some(d) = &w.digest
        && matches!(d.as_str(), "on" | "off")
    {
        m.insert(DIGEST_PREF_KEY.to_string(), Value::String(d.clone()));
    }
    if let Some(d) = &w.dnd {
        let dnd = obj_mut(&mut m, DND_KEY);
        if let Some(p) = &d.paused_until {
            dnd.insert(
                "pausedUntil".into(),
                p.clone().map(Value::String).unwrap_or(Value::Null),
            );
        }
        if let Some(s) = &d.schedule {
            dnd.insert(
                "schedule".into(),
                match s {
                    Some(s) => json!({
                        "enabled": s.enabled,
                        "days": s.days,
                        "start": s.start,
                        "end": s.end,
                    }),
                    None => Value::Null,
                },
            );
        }
        if let Some(a) = d.allow_needs_you {
            dnd.insert("allowNeedsYou".into(), Value::Bool(a));
        }
    }
    if let Some(f) = &w.email_frequency
        && EMAIL_FREQUENCIES.contains(&f.as_str())
    {
        obj_mut(&mut m, EMAIL_KEY).insert("frequency".into(), Value::String(f.clone()));
    }
    Value::Object(m)
}

/// Apply a write to the stored blob and return the blob after it. A row lock
/// for the read-modify-write: two tabs saving at once both land. Err
/// (RowNotFound) is the deleted-user corner.
pub async fn apply_notify_prefs(
    pg: &PgPool,
    user_id: &str,
    w: &PrefsWrite,
) -> Result<Value, sqlx::Error> {
    let mut tx = pg.begin().await?;
    let row: Option<(Option<Value>,)> =
        sqlx::query_as("select notify_prefs from users where id = $1::uuid for update")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((before,)) = row else {
        return Err(sqlx::Error::RowNotFound);
    };
    let after = merge_prefs(&before.unwrap_or(Value::Null), w);
    sqlx::query("update users set notify_prefs = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(&after)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(after)
}

pub(crate) async fn prefs_blob(pg: &PgPool, user_id: &str) -> Option<Value> {
    match sqlx::query_scalar::<_, Value>("select notify_prefs from users where id = $1::uuid")
        .bind(user_id)
        .fetch_optional(pg)
        .await
    {
        // No row and a NULL column are the same answer here — the defaults —
        // and sqlx maps both to None.
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[notifications] could not read notify_prefs: {e}");
            None
        }
    }
}

/// The stored blob and the person's time zone (`users.timezone`, else UTC),
/// from one row. Never fails: a failed read is the defaults.
pub async fn prefs_and_zone(pg: &PgPool, user_id: &str) -> (Value, String) {
    match sqlx::query_as::<_, (Option<Value>, Option<String>)>(
        "select notify_prefs, timezone from users where id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await
    {
        Ok(Some((blob, tz))) => (
            blob.unwrap_or(Value::Null),
            tz.map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "UTC".into()),
        ),
        Ok(None) => (Value::Null, "UTC".into()),
        Err(e) => {
            tracing::error!("[notifications] could not read notify_prefs: {e}");
            (Value::Null, "UTC".into())
        }
    }
}

/// Everything the Settings panel shows, from one row: the resolved table and
/// the effective digest answer. Never fails.
pub async fn get_notify_settings(pg: &PgPool, user_id: &str) -> (Value, &'static str) {
    let blob = prefs_blob(pg, user_id).await.unwrap_or(Value::Null);
    (
        resolve_prefs(&blob),
        if digest_enabled(&blob) { "on" } else { "off" },
    )
}

/// Merge a legacy patch into the stored blob and return the effective
/// settings. Unknown keys are dropped rather than stored — the column is jsonb
/// and this is the writer that keeps it honest. Err (no row) is the
/// deleted-user corner.
pub async fn set_notify_settings(
    pg: &PgPool,
    user_id: &str,
    prefs: Option<&serde_json::Map<String, Value>>,
    digest: Option<&str>,
) -> Result<(Value, &'static str), sqlx::Error> {
    let blob = apply_notify_prefs(
        pg,
        user_id,
        &PrefsWrite {
            prefs: prefs.cloned(),
            digest: digest.map(str::to_string),
            ..Default::default()
        },
    )
    .await?;
    Ok((
        resolve_prefs(&blob),
        if digest_enabled(&blob) { "on" } else { "off" },
    ))
}

// ── DND and working hours (I-14) ─────────────────────────────────────────────

/// "Until I resume": the pause that never ends on its own.
pub const FOREVER: &str = "9999-12-31T23:59:59.999Z";
/// FOREVER as epoch ms.
pub const FOREVER_MS: i64 = 253_402_300_799_999;

/// An instant in the spellings the wire accepts (RFC 3339 with `Z` or an
/// offset), as epoch ms. None for anything else.
pub fn parse_instant(s: &str) -> Option<i64> {
    talaria_tz::parse_rfc3339_ms(s.trim())
}

/// `HH:MM`, 00:00–23:59, as minutes past midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    if b.len() != 5
        || b[2] != b':'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 2 || c.is_ascii_digit())
    {
        return None;
    }
    let h = u32::from(b[0] - b'0') * 10 + u32::from(b[1] - b'0');
    let m = u32::from(b[3] - b'0') * 10 + u32::from(b[4] - b'0');
    (h < 24 && m < 60).then_some(h * 60 + m)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DndReason {
    /// A pause the person set ("for 1 hour", "until I resume").
    Paused,
    /// Outside their working hours.
    Schedule,
}

/// Is DND on now, until when, and why. THE one evaluator: the GET payload
/// (display) and W2-E's delivery (enforcement) both call `dnd_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DndState {
    pub active: bool,
    /// When it ends, epoch ms; None when inactive, or when a schedule with no
    /// working day at all never ends it.
    pub until: Option<i64>,
    pub reason: Option<DndReason>,
}

struct Hours {
    days: [bool; 7],
    start: u32,
    end: u32,
}

fn stored_schedule(dnd: &Value) -> Option<Hours> {
    let s = dnd.get("schedule")?;
    if s.get("enabled").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let start = parse_hhmm(s.get("start")?.as_str()?)?;
    let end = parse_hhmm(s.get("end")?.as_str()?)?;
    let mut days = [false; 7];
    for d in s.get("days")?.as_array()? {
        if let Some(d) = d.as_u64().filter(|d| *d < 7) {
            days[d as usize] = true;
        }
    }
    Some(Hours { days, start, end })
}

/// 0 = Sunday, from a local `YYYY-MM-DD`.
fn weekday_of(date: &str) -> Option<usize> {
    use chrono::Datelike;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|d| d.weekday().num_days_from_sunday() as usize)
}

/// Inside working hours at `at_ms`, read on the zone's own wall clock (DST
/// included). `zone` is already known good.
fn inside_hours(h: &Hours, zone: &str, at_ms: i64) -> bool {
    let m = talaria_tz::local_moment(zone, at_ms);
    let t = m.hour * 60 + m.minute;
    let Some(wd) = weekday_of(&m.date) else {
        return false;
    };
    let yesterday = (wd + 6) % 7;
    match h.start.cmp(&h.end) {
        std::cmp::Ordering::Less => h.days[wd] && h.start <= t && t < h.end,
        // Crosses midnight: the window that opened on a working day runs into
        // the next morning.
        std::cmp::Ordering::Greater => {
            (h.days[wd] && t >= h.start) || (h.days[yesterday] && t < h.end)
        }
        std::cmp::Ordering::Equal => h.days[wd],
    }
}

/// The first whole minute at or after `from_ms` inside working hours, within
/// eight days; None when there is none. Walks the zone's own clock in steps no
/// longer than the window (so no window is stepped over), then back to the
/// window's first minute.
fn next_inside(h: &Hours, zone: &str, from_ms: i64) -> Option<i64> {
    const MIN: i64 = 60_000;
    if !h.days.iter().any(|d| *d) {
        return None;
    }
    let window = match (h.end + 1440 - h.start) % 1440 {
        0 => 1440,
        w => w,
    };
    let step = i64::from(window.min(15)) * MIN;
    let base = from_ms - from_ms.rem_euclid(MIN) + MIN;
    let horizon = from_ms + 8 * 24 * 60 * MIN;
    let mut t = base;
    while t <= horizon {
        if inside_hours(h, zone, t) {
            while t - MIN > from_ms && inside_hours(h, zone, t - MIN) {
                t -= MIN;
            }
            return Some(t);
        }
        t += step;
    }
    None
}

/// DND at `now_ms` for the person whose blob this is, in `time_zone` (a bad
/// zone falls back to UTC, as `talaria-tz` does). Pure.
///
/// A pause wins while it lasts; when it ends outside working hours, DND runs
/// on to the next working minute. Outside working hours with no pause, DND is
/// on until they start again.
pub fn dnd_state(blob: &Value, time_zone: &str, now_ms: i64) -> DndState {
    dnd_state_in(blob, &resolve_zone(time_zone, now_ms), now_ms)
}

/// The zone `local_moment` will actually read in: the name given, or UTC for
/// one it does not know (it warns on every call with a bad one, so this
/// resolves it once per evaluation).
fn resolve_zone(time_zone: &str, now_ms: i64) -> String {
    talaria_tz::local_moment(time_zone, now_ms).zone
}

fn dnd_state_in(blob: &Value, zone: &str, now_ms: i64) -> DndState {
    let zone = zone.to_string();
    let dnd = blob.get(DND_KEY).unwrap_or(&Value::Null);
    let hours = stored_schedule(dnd);
    let paused = dnd
        .get("pausedUntil")
        .and_then(Value::as_str)
        .and_then(parse_instant)
        .filter(|until| *until > now_ms);
    if let Some(until) = paused {
        let until = match &hours {
            Some(h) if until < FOREVER_MS && !inside_hours(h, &zone, until) => {
                next_inside(h, &zone, until).or(Some(until))
            }
            _ => Some(until),
        };
        return DndState {
            active: true,
            until,
            reason: Some(DndReason::Paused),
        };
    }
    if let Some(h) = &hours
        && !inside_hours(h, &zone, now_ms)
    {
        return DndState {
            active: true,
            until: next_inside(h, &zone, now_ms),
            reason: Some(DndReason::Schedule),
        };
    }
    DndState {
        active: false,
        until: None,
        reason: None,
    }
}

/// `dnd` as the wire serves it: what is stored (pausedUntil, schedule,
/// allowNeedsYou — default true) and what it means now (active, until, reason)
/// in the zone it was read in.
pub fn dnd_wire(blob: &Value, time_zone: &str, now_ms: i64) -> Value {
    let dnd = blob.get(DND_KEY).unwrap_or(&Value::Null);
    let zone = resolve_zone(time_zone, now_ms);
    let state = dnd_state_in(blob, &zone, now_ms);
    let schedule = match dnd.get("schedule") {
        Some(s) if s.is_object() => json!({
            "enabled": s.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "days": s.get("days").cloned().filter(Value::is_array).unwrap_or_else(|| json!([])),
            "start": s.get("start").and_then(Value::as_str).unwrap_or("09:00"),
            "end": s.get("end").and_then(Value::as_str).unwrap_or("17:00"),
        }),
        _ => Value::Null,
    };
    json!({
        "pausedUntil": dnd.get("pausedUntil").and_then(Value::as_str),
        "schedule": schedule,
        "allowNeedsYou": dnd.get("allowNeedsYou").and_then(Value::as_bool).unwrap_or(true),
        "active": state.active,
        "until": state.until.map(epoch_ms_to_iso),
        "reason": state.reason,
        "timeZone": zone,
    })
}

// ── Missed-activity mail frequency ───────────────────────────────────────────

/// What `emailFrequency` may be. Stored and served here; W2-E's batcher reads
/// it.
pub const EMAIL_FREQUENCIES: &[&str] = &["15m", "1h", "1d", "off"];

/// The person's frequency, default hourly.
pub fn email_frequency(blob: &Value) -> &'static str {
    let stored = blob
        .get(EMAIL_KEY)
        .and_then(|e| e.get("frequency"))
        .and_then(Value::as_str);
    EMAIL_FREQUENCIES
        .iter()
        .find(|f| Some(**f) == stored)
        .copied()
        .unwrap_or("1h")
}

// ── The wire, from one row ───────────────────────────────────────────────────

/// Every settings key `GET`/`PATCH /api/notifications` serves, from one blob.
pub struct SettingsWire {
    pub prefs: Value,
    pub digest: &'static str,
    pub prompt: Value,
    pub channels: Value,
    pub dnd: Value,
    pub email_frequency: &'static str,
}

pub fn settings_wire(blob: &Value, time_zone: &str, now_ms: i64, is_admin: bool) -> SettingsWire {
    SettingsWire {
        prefs: resolve_prefs(blob),
        digest: if digest_enabled(blob) { "on" } else { "off" },
        prompt: prompt_wire(blob),
        channels: channels_wire(blob, is_admin),
        dnd: dnd_wire(blob, time_zone, now_ms),
        email_frequency: email_frequency(blob),
    }
}

// ── The soft ask's memory ────────────────────────────────────────────────────
//
// The in-app card that invites a person to turn desktop alerts on remembers
// its "Not now" ON THE ACCOUNT, not in one browser: a person who said no on
// their laptop is not asked again the same week on their desktop. It lives
// under the reserved `prompt` key of `users.notify_prefs` — beside the class
// routes and `digest`, read by none of them (`resolve_prefs` and
// `digest_enabled` read class keys only) — and only this writer touches it.
//
// The rule: the 1st "Not now" waits 7 days, the 2nd 30, and after the 3rd the
// card never shows again (the bell's own row stays). A grant ends it for
// good. Browsers embargo an origin whose native prompt is dismissed about
// three times, so a soft ask that kept coming back would spend that budget
// for us.

const PROMPT_KEY: &str = "prompt";
/// Dismissals after which the card is retired.
pub const PROMPT_MAX_DISMISSALS: i64 = 3;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// What the person answered the card (or the browser's own prompt) with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAction {
    /// "Not now": count one, and wait before asking again.
    Dismissed,
    /// The browser granted: never ask again.
    Granted,
}

/// The stored record after `action`, from the record before it (any shape —
/// a hand-edited value degrades to "never asked"). Pure, so the spacing is
/// pinned by a test.
pub fn next_prompt(prev: &Value, action: PromptAction, now_ms: i64) -> Value {
    let count = prev
        .get("count")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let granted = prev
        .get("granted")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let dismissed_at = prev.get("dismissedAt").cloned().unwrap_or(Value::Null);
    match action {
        PromptAction::Granted => serde_json::json!({
            "count": count,
            "dismissedAt": dismissed_at,
            "nextAskAt": Value::Null,
            "granted": true,
        }),
        PromptAction::Dismissed => {
            let count = (count + 1).min(PROMPT_MAX_DISMISSALS);
            let next = match count {
                1 => Value::String(epoch_ms_to_iso(now_ms + 7 * DAY_MS)),
                2 => Value::String(epoch_ms_to_iso(now_ms + 30 * DAY_MS)),
                _ => Value::Null,
            };
            serde_json::json!({
                "count": count,
                "dismissedAt": epoch_ms_to_iso(now_ms),
                "nextAskAt": next,
                "granted": granted,
            })
        }
    }
}

/// The wire shape `GET /api/notifications` serves as `prompt`:
/// `{dismissedAt, count, nextAskAt, granted}`, from whatever the blob holds.
/// `nextAskAt` null with `count` under the cap means "may ask now".
pub fn prompt_wire(blob: &Value) -> Value {
    let p = blob.get(PROMPT_KEY).unwrap_or(&Value::Null);
    let text = |k: &str| {
        p.get(k)
            .and_then(Value::as_str)
            .map(|s| Value::String(s.to_string()))
            .unwrap_or(Value::Null)
    };
    serde_json::json!({
        "dismissedAt": text("dismissedAt"),
        "count": p.get("count").and_then(Value::as_i64).unwrap_or(0).clamp(0, PROMPT_MAX_DISMISSALS),
        "nextAskAt": text("nextAskAt"),
        "granted": p.get("granted").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// The Settings read plus the soft ask's memory, from ONE row read: the
/// resolved table, the effective digest answer, and `prompt_wire`.
pub async fn get_notify_settings_and_prompt(
    pg: &PgPool,
    user_id: &str,
) -> (Value, &'static str, Value) {
    let blob = prefs_blob(pg, user_id).await.unwrap_or(Value::Null);
    (
        resolve_prefs(&blob),
        if digest_enabled(&blob) { "on" } else { "off" },
        prompt_wire(&blob),
    )
}

/// Record a soft-ask answer and return the new wire record. Read, compute,
/// merge: a person answers one card at a time, so the read-modify-write has
/// no race worth a lock. Err (no row) is the deleted-user corner.
pub async fn set_notify_prompt(
    pg: &PgPool,
    user_id: &str,
    action: PromptAction,
) -> Result<Value, sqlx::Error> {
    let blob = prefs_blob(pg, user_id).await.unwrap_or(Value::Null);
    let prev = blob.get(PROMPT_KEY).cloned().unwrap_or(Value::Null);
    let next = next_prompt(&prev, action, wall_ms());
    let row: Option<(Value,)> = sqlx::query_as(
        "update users set notify_prefs = coalesce(notify_prefs, '{}'::jsonb) || jsonb_build_object('prompt', $2::jsonb) \
         where id = $1::uuid returning notify_prefs",
    )
    .bind(user_id)
    .bind(&next)
    .fetch_optional(pg)
    .await?;
    let Some((blob,)) = row else {
        return Err(sqlx::Error::RowNotFound);
    };
    Ok(prompt_wire(&blob))
}

/// Is this instance sending notification mail? Off unless the settings row
/// says exactly true — a hand-edited row must not be able to turn mail on.
/// get_setting already falls back on a failed read, so this never errors.
pub async fn get_notify_delivery(pg: &PgPool) -> bool {
    let stored = get_setting(pg, DELIVERY_KEY, serde_json::json!({})).await;
    stored.get("emailEnabled") == Some(&Value::Bool(true))
}

/// Turn notification email on or off for the whole instance — the admin
/// master switch. The 10s in-process read cache is not wire-visible; the DB
/// row is the contract.
///
/// MERGES (C28): the row may carry other delivery switches beside
/// `emailEnabled` (W2-E adds `pushEnabled`), and flipping one must not erase
/// the others. A non-object row is replaced. A read that FAILS fails the
/// write — merging into a fallback `{}` would be exactly the overwrite this
/// exists to prevent.
pub async fn set_notify_delivery(pg: &PgPool, email_enabled: bool) -> Result<(), sqlx::Error> {
    let stored = talaria_settings::try_get_setting(pg, DELIVERY_KEY)
        .await?
        .unwrap_or_else(|| serde_json::json!({}));
    set_setting(pg, DELIVERY_KEY, &merge_delivery(&stored, email_enabled)).await
}

/// The delivery row after an email flip, from the row before it. Pure.
pub fn merge_delivery(stored: &Value, email_enabled: bool) -> Value {
    let mut m = stored.as_object().cloned().unwrap_or_default();
    m.insert("emailEnabled".into(), Value::Bool(email_enabled));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // The soft ask's spacing: 7 days, then 30, then never; a grant ends it.
    #[test]
    fn the_prompt_waits_seven_then_thirty_days_then_retires() {
        let t0 = 1_700_000_000_000i64;
        let one = next_prompt(&Value::Null, PromptAction::Dismissed, t0);
        assert_eq!(one["count"], json!(1));
        assert_eq!(one["nextAskAt"], json!(epoch_ms_to_iso(t0 + 7 * DAY_MS)));
        assert_eq!(one["dismissedAt"], json!(epoch_ms_to_iso(t0)));
        let two = next_prompt(&one, PromptAction::Dismissed, t0 + 8 * DAY_MS);
        assert_eq!(two["count"], json!(2));
        assert_eq!(two["nextAskAt"], json!(epoch_ms_to_iso(t0 + 38 * DAY_MS)));
        let three = next_prompt(&two, PromptAction::Dismissed, t0 + 40 * DAY_MS);
        assert_eq!(three["count"], json!(3));
        assert_eq!(three["nextAskAt"], Value::Null);
        // Past the cap it stays retired.
        let four = next_prompt(&three, PromptAction::Dismissed, t0 + 41 * DAY_MS);
        assert_eq!(four["count"], json!(3));
        assert_eq!(four["nextAskAt"], Value::Null);
        // A grant stops it for good, keeping the history.
        let granted = next_prompt(&one, PromptAction::Granted, t0 + DAY_MS);
        assert_eq!(granted["granted"], json!(true));
        assert_eq!(granted["count"], json!(1));
        assert_eq!(granted["nextAskAt"], Value::Null);
    }

    #[test]
    fn the_prompt_wire_degrades_to_never_asked() {
        assert_eq!(
            prompt_wire(&Value::Null),
            json!({ "dismissedAt": null, "count": 0, "nextAskAt": null, "granted": false })
        );
        assert_eq!(
            prompt_wire(&json!({ "prompt": "junk" })),
            json!({ "dismissedAt": null, "count": 0, "nextAskAt": null, "granted": false })
        );
        assert_eq!(
            prompt_wire(&json!({ "prompt": { "count": 9, "nextAskAt": 5 } })),
            json!({ "dismissedAt": null, "count": 3, "nextAskAt": null, "granted": false })
        );
        // The reserved key changes neither routes nor the digest answer.
        let blob = json!({ "prompt": { "count": 1 }, "mention": "in_app" });
        assert_eq!(resolve_prefs(&blob)["mention"], json!("in_app"));
        assert_eq!(
            digest_enabled(&blob),
            digest_enabled(&json!({ "mention": "in_app" }))
        );
    }

    #[test]
    fn resolve_fills_defaults_in_class_order() {
        // Untouched user: every fallback, in NOTIFY_CLASSES order.
        let resolved = resolve_prefs(&json!({}));
        let keys: Vec<&String> = resolved.as_object().unwrap().keys().collect();
        assert_eq!(
            keys.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
            NOTIFY_CLASSES.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        );
        assert_eq!(resolved["mention"], json!("both"));
        assert_eq!(resolved["gap_reported"], json!("in_app"));
        // A stored route wins; a garbage value is dropped to the fallback.
        let stored = json!({ "mention": "in_app", "dm": "email", "gap_reported": "nope" });
        let resolved = resolve_prefs(&stored);
        assert_eq!(resolved["mention"], json!("in_app"));
        assert_eq!(resolved["dm"], json!("email"));
        assert_eq!(resolved["gap_reported"], json!("in_app"));
        // A hand-edited non-class key is ignored, and null storage is the
        // untouched case.
        let stored = json!({ "bogus": "both" });
        assert_eq!(resolve_prefs(&stored)["work_complete"], json!("in_app"));
        assert_eq!(resolve_prefs(&Value::Null)["mention"], json!("both"));
    }

    #[test]
    fn digest_explicit_wins_then_derives_away_from_spam() {
        // Explicit on/off wins outright, both directions.
        assert!(digest_enabled(&json!({ "digest": "on" })));
        assert!(!digest_enabled(&json!({ "digest": "off" })));
        // No explicit choice derives: any class not in_app means mail.
        assert!(digest_enabled(&json!({})));
        assert!(digest_enabled(&json!({ "mention": "in_app" })));
        // Every class routed in-app is "do not email me" — no digest either.
        let all_in_app = json!({
            "mention": "in_app", "dm": "in_app", "approval_pending": "in_app",
            "judge_escalation": "in_app", "agent_blocked": "in_app",
            "gap_reported": "in_app", "work_complete": "in_app",
            "workchain_turn": "in_app", "workchain_paused": "in_app"
        });
        assert!(!digest_enabled(&all_in_app));
        // A garbage digest value falls back to the derivation, not to off.
        assert!(digest_enabled(&json!({ "digest": "garbage" })));
    }

    // ── prefs v2 ─────────────────────────────────────────────────────────────

    fn w() -> PrefsWrite {
        PrefsWrite::default()
    }

    // For every legacy blob wave 1 could have stored, the channels derive the
    // same route back: phase 0 changes no one's routing table.
    #[test]
    fn legacy_blobs_round_trip_through_channels() {
        for route in ["in_app", "email", "both"] {
            for (id, _) in NOTIFY_CLASSES {
                let blob = json!({ *id: route });
                assert_eq!(resolve_prefs(&blob)[*id], json!(route), "{id} {route}");
            }
        }
        let c = channels_of(&json!({ "dm": "email" }), "dm");
        assert_eq!(
            c,
            Channels {
                in_app: true,
                desktop: false,
                push: false,
                email: true
            }
        );
        let c = channels_of(&json!({}), "work_complete");
        assert_eq!(
            c,
            Channels {
                in_app: true,
                desktop: true,
                push: true,
                email: false
            }
        );
        // The new class has no legacy key: its registry defaults.
        let c = channels_of(&json!({}), "channel_activity");
        assert_eq!(
            c,
            Channels {
                in_app: true,
                desktop: true,
                push: false,
                email: false
            }
        );
    }

    #[test]
    fn a_v2_write_rewrites_the_legacy_route() {
        let mut v2 = w();
        v2.channels = Some(vec![(
            "mention".into(),
            ChannelsPatch {
                email: Some(false),
                ..Default::default()
            },
        )]);
        let blob = merge_prefs(&json!({}), &v2);
        assert_eq!(blob["mention"], json!("in_app"));
        assert_eq!(blob["channels"]["mention"], json!({ "email": false }));
        assert_eq!(resolve_prefs(&blob)["mention"], json!("in_app"));
        // Email on, both screens off → email.
        v2.channels = Some(vec![(
            "dm".into(),
            ChannelsPatch {
                desktop: Some(false),
                push: Some(false),
                email: Some(true),
            },
        )]);
        let blob = merge_prefs(&blob, &v2);
        assert_eq!(blob["dm"], json!("email"));
        // Email on with a screen → both; the earlier class is untouched.
        v2.channels = Some(vec![(
            "dm".into(),
            ChannelsPatch {
                push: Some(true),
                ..Default::default()
            },
        )]);
        let blob = merge_prefs(&blob, &v2);
        assert_eq!(blob["dm"], json!("both"));
        assert_eq!(blob["mention"], json!("in_app"));
        // A new class gets channels but no legacy key; an unknown class is dropped.
        v2.channels = Some(vec![
            (
                "participating".into(),
                ChannelsPatch {
                    email: Some(true),
                    ..Default::default()
                },
            ),
            (
                "bogus".into(),
                ChannelsPatch {
                    email: Some(true),
                    ..Default::default()
                },
            ),
        ]);
        let blob = merge_prefs(&blob, &v2);
        assert!(blob.get("participating").is_none());
        assert_eq!(blob["channels"]["participating"]["email"], json!(true));
        assert!(blob["channels"].get("bogus").is_none());
        assert!(blob.get("bogus").is_none());
    }

    #[test]
    fn a_legacy_write_from_a_stale_bundle_is_still_obeyed() {
        // A person who turned desktop off in the matrix, then a stale bundle
        // routes mention to email: email on, both screens off.
        let mut v2 = w();
        v2.channels = Some(vec![(
            "mention".into(),
            ChannelsPatch {
                desktop: Some(false),
                ..Default::default()
            },
        )]);
        let blob = merge_prefs(&json!({}), &v2);
        let mut legacy = w();
        legacy.prefs = Some(json!({ "mention": "email" }).as_object().unwrap().clone());
        let blob = merge_prefs(&blob, &legacy);
        assert_eq!(blob["mention"], json!("email"));
        assert_eq!(
            channels_of(&blob, "mention"),
            Channels {
                in_app: true,
                desktop: false,
                push: false,
                email: true
            }
        );
        // Back to both: email stays on, the stored desktop=false is kept, and
        // push (stored false by the email write) — both were off, so both
        // come back on: `both` means "on my screens too".
        legacy.prefs = Some(json!({ "mention": "both" }).as_object().unwrap().clone());
        let blob = merge_prefs(&blob, &legacy);
        assert_eq!(
            channels_of(&blob, "mention"),
            Channels {
                in_app: true,
                desktop: true,
                push: true,
                email: true
            }
        );
        assert_eq!(resolve_prefs(&blob)["mention"], json!("both"));
        // in_app keeps a stored screen choice and turns email off.
        let mut v2 = w();
        v2.channels = Some(vec![(
            "dm".into(),
            ChannelsPatch {
                push: Some(false),
                ..Default::default()
            },
        )]);
        let blob = merge_prefs(&blob, &v2);
        legacy.prefs = Some(
            json!({ "dm": "in_app", "bogus": "both" })
                .as_object()
                .unwrap()
                .clone(),
        );
        let blob = merge_prefs(&blob, &legacy);
        assert_eq!(
            channels_of(&blob, "dm"),
            Channels {
                in_app: true,
                desktop: true,
                push: false,
                email: false
            }
        );
        assert!(blob.get("bogus").is_none());
    }

    // C27: the digest's derivation reads the original nine only.
    #[test]
    fn the_digest_is_frozen_to_the_original_classes() {
        let mut off = json!({});
        for id in ORIGINAL_CLASSES {
            off[*id] = json!("in_app");
        }
        assert!(!digest_enabled(&off));
        let mut v2 = w();
        v2.channels = Some(vec![(
            "participating".into(),
            ChannelsPatch {
                email: Some(true),
                ..Default::default()
            },
        )]);
        let blob = merge_prefs(&off, &v2);
        assert!(!digest_enabled(&blob));
        // …and an original class with email on still derives it on.
        v2.channels = Some(vec![(
            "dm".into(),
            ChannelsPatch {
                email: Some(true),
                ..Default::default()
            },
        )]);
        assert!(digest_enabled(&merge_prefs(&blob, &v2)));
    }

    #[test]
    fn email_frequency_and_delivery_merge() {
        assert_eq!(email_frequency(&json!({})), "1h");
        let mut f = w();
        f.email_frequency = Some("15m".into());
        assert_eq!(email_frequency(&merge_prefs(&json!({}), &f)), "15m");
        assert_eq!(
            email_frequency(&json!({ "email": { "frequency": "9h" } })),
            "1h"
        );
        // C28: flipping email keeps any other switch on the row.
        assert_eq!(
            merge_delivery(&json!({ "emailEnabled": false, "pushEnabled": true }), true),
            json!({ "emailEnabled": true, "pushEnabled": true })
        );
        assert_eq!(
            merge_delivery(&json!("junk"), false),
            json!({ "emailEnabled": false })
        );
    }

    // ── DND ─────────────────────────────────────────────────────────────────

    fn at(iso: &str) -> i64 {
        talaria_agent_auth::iso_to_epoch_ms(iso).unwrap()
    }

    fn sched(days: &[u8], start: &str, end: &str) -> Value {
        json!({ "dnd": { "schedule": { "enabled": true, "days": days, "start": start, "end": end } } })
    }

    const WEEKDAYS: &[u8] = &[1, 2, 3, 4, 5];

    #[test]
    fn a_pause_is_active_until_it_ends() {
        let now = at("2026-10-31T12:00:00.000Z");
        let blob = json!({ "dnd": { "pausedUntil": "2026-10-31T13:00:00.000Z" } });
        let s = dnd_state(&blob, "UTC", now);
        assert_eq!(
            s,
            DndState {
                active: true,
                until: Some(at("2026-10-31T13:00:00.000Z")),
                reason: Some(DndReason::Paused)
            }
        );
        // Past its end it is off; an offset spelling reads the same instant.
        assert!(!dnd_state(&blob, "UTC", at("2026-10-31T13:00:00.000Z")).active);
        let off = json!({ "dnd": { "pausedUntil": "2026-10-31T09:00:00-04:00" } });
        assert_eq!(
            dnd_state(&off, "UTC", now).until,
            Some(at("2026-10-31T13:00:00.000Z"))
        );
        // Until I resume.
        let forever = json!({ "dnd": { "pausedUntil": FOREVER } });
        assert_eq!(dnd_state(&forever, "UTC", now).until, Some(FOREVER_MS));
        assert_eq!(parse_instant(FOREVER), Some(FOREVER_MS));
        // Nothing stored, or garbage: off.
        assert!(!dnd_state(&json!({}), "UTC", now).active);
        assert!(!dnd_state(&json!({ "dnd": { "pausedUntil": "soon" } }), "UTC", now).active);
    }

    #[test]
    fn working_hours_edges_at_both_ends() {
        // 2026-11-02 is a Monday.
        let blob = sched(WEEKDAYS, "09:00", "17:00");
        let mon = |hm: &str| at(&format!("2026-11-02T{hm}:00.000Z"));
        // 08:59 is outside (quiet until 09:00), 09:00 inside, 16:59 inside,
        // 17:00 outside (quiet until Tuesday 09:00).
        let s = dnd_state(&blob, "UTC", mon("08:59"));
        assert_eq!(
            s,
            DndState {
                active: true,
                until: Some(mon("09:00")),
                reason: Some(DndReason::Schedule)
            }
        );
        assert!(!dnd_state(&blob, "UTC", mon("09:00")).active);
        assert!(!dnd_state(&blob, "UTC", mon("16:59")).active);
        let s = dnd_state(&blob, "UTC", mon("17:00"));
        assert_eq!(s.until, Some(at("2026-11-03T09:00:00.000Z")));
        // Saturday morning waits for Monday.
        let s = dnd_state(&blob, "UTC", at("2026-10-31T10:00:00.000Z"));
        assert_eq!(s.until, Some(mon("09:00")));
        // A disabled schedule, or one with no days, is respectively off and
        // never-ending.
        let mut off = blob.clone();
        off["dnd"]["schedule"]["enabled"] = json!(false);
        assert!(!dnd_state(&off, "UTC", mon("08:00")).active);
        let none = sched(&[], "09:00", "17:00");
        assert_eq!(
            dnd_state(&none, "UTC", mon("12:00")),
            DndState {
                active: true,
                until: None,
                reason: Some(DndReason::Schedule)
            }
        );
    }

    #[test]
    fn an_overnight_window_crosses_midnight() {
        // A night shift, Monday 22:00 → Tuesday 06:00.
        let blob = sched(&[1], "22:00", "06:00");
        assert!(!dnd_state(&blob, "UTC", at("2026-11-02T23:30:00.000Z")).active);
        assert!(!dnd_state(&blob, "UTC", at("2026-11-03T05:59:00.000Z")).active);
        let s = dnd_state(&blob, "UTC", at("2026-11-03T06:00:00.000Z"));
        assert_eq!(s.until, Some(at("2026-11-09T22:00:00.000Z")));
        // Monday morning before the shift is outside it.
        assert!(dnd_state(&blob, "UTC", at("2026-11-02T05:00:00.000Z")).active);
    }

    #[test]
    fn working_hours_follow_the_zone_across_dst() {
        // New York falls back on Sunday 2026-11-01; Monday 09:00 EST is 14:00Z
        // (it was 13:00Z the Monday before, under EDT).
        let blob = sched(WEEKDAYS, "09:00", "17:00");
        let s = dnd_state(&blob, "America/New_York", at("2026-10-31T16:00:00.000Z"));
        assert_eq!(s.until, Some(at("2026-11-02T14:00:00.000Z")));
        let s = dnd_state(&blob, "America/New_York", at("2026-10-24T16:00:00.000Z"));
        assert_eq!(s.until, Some(at("2026-10-26T13:00:00.000Z")));
        // London springs forward on 2026-03-29: Monday 09:00 BST is 08:00Z.
        let s = dnd_state(&blob, "Europe/London", at("2026-03-28T12:00:00.000Z"));
        assert_eq!(s.until, Some(at("2026-03-30T08:00:00.000Z")));
        // And at 08:30Z that Monday London is inside (09:30 BST); New York is not.
        assert!(!dnd_state(&blob, "Europe/London", at("2026-03-30T08:30:00.000Z")).active);
        assert!(dnd_state(&blob, "America/New_York", at("2026-03-30T08:30:00.000Z")).active);
    }

    #[test]
    fn a_bad_zone_reads_as_utc() {
        let blob = sched(WEEKDAYS, "09:00", "17:00");
        let mon = at("2026-11-02T10:00:00.000Z");
        assert_eq!(
            dnd_state(&blob, "Not/AZone", mon),
            dnd_state(&blob, "UTC", mon)
        );
        assert_eq!(dnd_wire(&blob, "Not/AZone", mon)["timeZone"], json!("UTC"));
    }

    #[test]
    fn a_pause_that_ends_outside_hours_runs_on_to_them() {
        let blob = json!({ "dnd": {
            "pausedUntil": "2026-11-02T18:00:00.000Z",
            "schedule": { "enabled": true, "days": WEEKDAYS, "start": "09:00", "end": "17:00" }
        } });
        let s = dnd_state(&blob, "UTC", at("2026-11-02T12:00:00.000Z"));
        assert_eq!(
            s,
            DndState {
                active: true,
                until: Some(at("2026-11-03T09:00:00.000Z")),
                reason: Some(DndReason::Paused)
            }
        );
    }

    #[test]
    fn the_dnd_wire_carries_stored_and_computed() {
        let mut d = w();
        d.dnd = Some(DndPatch {
            paused_until: Some(Some("2026-10-31T13:00:00.000Z".into())),
            schedule: Some(Some(DndSchedule {
                enabled: true,
                days: vec![1, 2, 3, 4, 5],
                start: "09:00".into(),
                end: "17:00".into(),
            })),
            allow_needs_you: None,
        });
        let blob = merge_prefs(&json!({}), &d);
        let wire = dnd_wire(&blob, "UTC", at("2026-10-31T12:00:00.000Z"));
        assert_eq!(
            wire,
            json!({
                "pausedUntil": "2026-10-31T13:00:00.000Z",
                "schedule": { "enabled": true, "days": [1, 2, 3, 4, 5], "start": "09:00", "end": "17:00" },
                "allowNeedsYou": true,
                "active": true,
                "until": "2026-11-02T09:00:00.000Z",
                "reason": "paused",
                "timeZone": "UTC",
            })
        );
        // Clearing both.
        d.dnd = Some(DndPatch {
            paused_until: Some(None),
            schedule: Some(None),
            allow_needs_you: Some(false),
        });
        let wire = dnd_wire(
            &merge_prefs(&blob, &d),
            "UTC",
            at("2026-10-31T12:00:00.000Z"),
        );
        assert_eq!(wire["pausedUntil"], Value::Null);
        assert_eq!(wire["schedule"], Value::Null);
        assert_eq!(wire["allowNeedsYou"], json!(false));
        assert_eq!(wire["active"], json!(false));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("9:00"), None);
        assert_eq!(parse_hhmm("23:59"), Some(1439));
    }
}
