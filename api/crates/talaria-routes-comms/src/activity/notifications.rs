// /api/notifications. The caller's inbox: GET is the bell's one read (list,
// unread, prefs, digest, the instance switch, whether THIS user may flip it,
// and the desktop-alerts soft ask's memory — and, since the contract, the v2
// rows, the For-you count, the notify seq read in the same statement as the
// rows, the class registry, the channel matrix, DND, the mail frequency and
// the feature flags), PUT marks read — by ids, by href, by entity, or all of
// them — or moves rows to Done and back, or marks them unread, and tells the
// person's other tabs and devices which rows, and PATCH changes routing, the
// matrix, DND or the frequency, or records a soft-ask answer. The mail
// fan-out behind the prefs (the outbox, the drain) is scheduler plane, so it
// is not here; this file ends at the rows and the switches the SPA reads and
// writes. Every key wave 1 served is still served, in order.
//
// GET's parameters, all optional; with none it is wave 1's newest 50, every
// row (docs/notifications/live-and-inbox.md):
//   view=foryou|all|done   For you (Needs you + Direct) and All leave Done
//                          rows out; Done is them alone, last archived first
//   unread=1               unread rows only
//   category=, class=, kind=   comma lists, each an OR, together an AND
//   limit=1..100           the page (default 50)
//   before=<next>          the next page: the previous answer's `next`
//   since=<seq>            instead of a page: every row with a higher seq, in
//                          seq order, ≤ 200, with `more` (I-19); the view
//                          parameters are ignored
// `next` is the following page's cursor, null at the end; `more` says whether
// rows remain past this answer.
//
// PUT's members: a selector — `ids` (≤ 200), `href`, or `entity: {type, id}`
// (the caller's own rows only), in that precedence; none (or empty ones) is
// every row — and at most one action: `archive: true` (Done, which also marks
// read), `unarchive: true`, `unread: true` (the last two need a selector).
// With no action it marks read. Every path publishes the read-sync event
// (I-16): a read or an archive names its rows in `readIds`; an unarchive or a
// mark-unread publishes the counts.
//
// The admin gate is a role check on an ALREADY-required user, ordered after
// the body's 400s and before any write, so a member who sends prefs and
// delivery in one PATCH gets 403 and NEITHER change — one PATCH can never
// half-apply.

use axum::Json;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use talaria_agent_auth::now_ms as wall_ms;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    boolean_member, boolean_msg, enum_member, enum_msg, literal_true_member, object_msg,
    optional_boolean_member, optional_max_string_member, optional_uuid_array_member, parse,
    record_msg, string_member, utf16_len, zod_type_name, zod_uuid_ok,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{
    ChannelsPatch, Cursor, DndPatch, DndSchedule, EMAIL_FREQUENCIES, ENTITY_TYPES, FOREVER_MS,
    InboxQuery, InboxView, NOTIFY_CLASSES, NotifyDeps, NotifyEntity, PAGE_MAX, PrefsWrite,
    PromptAction, Selector, apply_notify_prefs, archive_notifications, class_def, classes_wire,
    clear_entity, entity_type, features, get_notify_delivery, list_page, list_since,
    mark_brief_stale, mark_notifications_read, mark_notifications_unread, parse_category,
    parse_hhmm, parse_instant, prefs_and_zone, publish_read, publish_unread, set_notify_delivery,
    set_notify_prompt, settings_wire, unarchive_notifications, unread_counts,
};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;

const ROUTE_OPTS: &[&str] = &["in_app", "email", "both"];
const DIGEST_OPTS: &[&str] = &["on", "off"];

/// The validated PrefsPatch: each field Option<"present">, which is the very
/// thing the at-least-one check counts.
#[derive(Debug)]
struct PrefsPatch {
    prefs: Option<serde_json::Map<String, Value>>,
    digest: Option<String>,
    delivery: Option<bool>,
    /// A soft-ask answer: `{dismissed: true}` ("Not now") or
    /// `{granted: true}` (the browser said yes).
    prompt: Option<PromptAction>,
    /// The matrix, class → channels (I-13).
    channels: Option<Vec<(String, ChannelsPatch)>>,
    /// DND and working hours (I-14).
    dnd: Option<DndPatch>,
    email_frequency: Option<String>,
}

const CHANNEL_KEYS: &[&str] = &["desktop", "push", "email"];

/// The `channels` member: a record of class → `{desktop?, push?, email?}`.
/// In-app is not a choice — the row is the record — so `inApp: false` is
/// refused; `inApp: true` is the truth and passes.
fn channels_member(v: &Value) -> Result<Vec<(String, ChannelsPatch)>, String> {
    let m = v.as_object().ok_or_else(|| record_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(m.len());
    for (k, val) in m {
        if utf16_len(k) > 40 {
            return Err("Invalid key in record".into());
        }
        let c = val
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(val)))?;
        match c.get("inApp") {
            None | Some(Value::Bool(true)) => {}
            Some(Value::Bool(false)) => return Err("In-app is always on".into()),
            Some(other) => return Err(boolean_msg(zod_type_name(other))),
        }
        let mut p = ChannelsPatch::default();
        for key in CHANNEL_KEYS {
            let b = optional_boolean_member(c, key)?;
            match *key {
                "desktop" => p.desktop = b,
                "push" => p.push = b,
                _ => p.email = b,
            }
        }
        out.push((k.clone(), p));
    }
    if out.is_empty() {
        return Err("nothing to update".into());
    }
    if !out.iter().all(|(k, _)| class_def(k).is_some()) {
        return Err("unknown notification class".into());
    }
    Ok(out)
}

/// The `dnd` member: `{pausedUntil?: ISO|null, schedule?: {enabled, days,
/// start, end}|null, allowNeedsYou?: bool}`. A pause is an instant up to
/// "until I resume"; working hours are weekday numbers (0 = Sunday) and two
/// `HH:MM` times.
fn dnd_member(v: &Value) -> Result<DndPatch, String> {
    let d = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
    let mut p = DndPatch::default();
    if let Some(pu) = d.get("pausedUntil") {
        p.paused_until = Some(match pu {
            Value::Null => None,
            Value::String(s) if parse_instant(s).is_some_and(|ms| ms <= FOREVER_MS) => {
                Some(s.trim().to_string())
            }
            _ => return Err("pausedUntil must be an ISO time, or null".into()),
        });
    }
    if let Some(sv) = d.get("schedule") {
        p.schedule = Some(match sv {
            Value::Null => None,
            Value::Object(s) => {
                let enabled = boolean_member(s, "enabled")?;
                let days = s
                    .get("days")
                    .and_then(Value::as_array)
                    .ok_or("days must be weekday numbers 0-6")?;
                let mut out: Vec<u8> = Vec::with_capacity(days.len());
                for day in days {
                    match day.as_u64() {
                        Some(n) if n < 7 && !out.contains(&(n as u8)) => out.push(n as u8),
                        _ => return Err("days must be weekday numbers 0-6".into()),
                    }
                }
                out.sort_unstable();
                let hhmm = |k: &str| -> Result<String, String> {
                    s.get(k)
                        .and_then(Value::as_str)
                        .filter(|t| parse_hhmm(t).is_some())
                        .map(str::to_string)
                        .ok_or_else(|| format!("{k} must be HH:MM"))
                };
                Some(DndSchedule {
                    enabled,
                    days: out,
                    start: hhmm("start")?,
                    end: hhmm("end")?,
                })
            }
            other => return Err(object_msg(zod_type_name(other))),
        });
    }
    p.allow_needs_you = optional_boolean_member(d, "allowNeedsYou")?;
    if p == DndPatch::default() {
        return Err("nothing to update".into());
    }
    Ok(p)
}

/// The `prompt` member: an object carrying exactly one of `dismissed: true`
/// or `granted: true`.
fn prompt_member(v: &Value) -> Result<PromptAction, String> {
    let p = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
    let flag = |k: &str| p.get(k) == Some(&Value::Bool(true));
    match (flag("dismissed"), flag("granted")) {
        (true, false) => Ok(PromptAction::Dismissed),
        (false, true) => Ok(PromptAction::Granted),
        _ => Err("prompt must be {dismissed: true} or {granted: true}".into()),
    }
}

/// The validated PATCH body, checks in schema order: prefs (the record, then
/// its two refines), digest, delivery — then the at-least-one check. Every
/// message is pinned in the test at the bottom of this file.
fn validate_prefs_patch(obj: &serde_json::Map<String, Value>) -> Result<PrefsPatch, String> {
    let prefs = match obj.get("prefs") {
        None => None,
        Some(v) => {
            // The prefs record: entry by entry in document order, key then
            // value. A key past 40 chars is the GENERIC record-key message
            // (not the string-max spelling), and a value that is not exactly
            // a route is the enum's message whatever its JSON type.
            let m = v.as_object().ok_or_else(|| record_msg(zod_type_name(v)))?;
            for (k, val) in m {
                if utf16_len(k) > 40 {
                    return Err("Invalid key in record".into());
                }
                if !val.as_str().is_some_and(|s| ROUTE_OPTS.contains(&s)) {
                    return Err(enum_msg(ROUTE_OPTS));
                }
            }
            // non-empty before all-known — the field's own checks outrank
            // the body-wide at-least-one, so `{}` with a valid digest beside
            // it still answers the first one's message.
            if m.is_empty() {
                return Err("nothing to update".into());
            }
            if !m
                .keys()
                .all(|k| NOTIFY_CLASSES.iter().any(|(id, _)| id == k))
            {
                return Err("unknown notification class".into());
            }
            Some(m.clone())
        }
    };
    let digest = match obj.get("digest") {
        None => None,
        Some(_) => Some(enum_member(obj, "digest", DIGEST_OPTS)?),
    };
    let delivery = match obj.get("delivery") {
        None => None,
        Some(v) => {
            let d = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
            Some(boolean_member(d, "emailEnabled")?)
        }
    };
    let prompt = match obj.get("prompt") {
        None => None,
        Some(v) => Some(prompt_member(v)?),
    };
    let channels = match obj.get("channels") {
        None => None,
        Some(v) => Some(channels_member(v)?),
    };
    let dnd = match obj.get("dnd") {
        None => None,
        Some(v) => Some(dnd_member(v)?),
    };
    let email_frequency = match obj.get("emailFrequency") {
        None => None,
        Some(_) => Some(enum_member(obj, "emailFrequency", EMAIL_FREQUENCIES)?),
    };
    if prefs.is_none()
        && digest.is_none()
        && delivery.is_none()
        && prompt.is_none()
        && channels.is_none()
        && dnd.is_none()
        && email_frequency.is_none()
    {
        return Err("nothing to update".into());
    }
    Ok(PrefsPatch {
        prefs,
        digest,
        delivery,
        prompt,
        channels,
        dnd,
        email_frequency,
    })
}

async fn delivery_or_off(pg: &sqlx::PgPool) -> bool {
    get_notify_delivery(pg).await
}

/// What a GET asks for: a page of a view, or everything after a seq.
#[derive(Debug, PartialEq)]
enum ListRead {
    Page(InboxQuery),
    Since(i64),
}

/// The most entries a `kind=` list takes, and the longest kind in it.
const KIND_LIST_MAX: usize = 50;
const KIND_MAX: usize = 64;

/// A comma list parameter: trimmed, empties dropped; an empty list is the
/// parameter absent.
fn list_param(v: &str) -> Option<Vec<String>> {
    let items: Vec<String> = v
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (!items.is_empty()).then_some(items)
}

/// `next` on the wire: opaque, URL-safe. Inside, the order key to the
/// microsecond and the row id.
fn encode_cursor(c: &Cursor) -> String {
    URL_SAFE_NO_PAD.encode(format!("{}.{}", c.at_us, c.id))
}

fn decode_cursor(s: &str) -> Option<Cursor> {
    let raw = String::from_utf8(URL_SAFE_NO_PAD.decode(s.trim()).ok()?).ok()?;
    let (at, id) = raw.split_once('.')?;
    let at_us: i64 = at.parse().ok()?;
    zod_uuid_ok(id).then(|| Cursor {
        at_us,
        id: id.to_string(),
    })
}

/// The GET's query string, checked. The last spelling of a repeated
/// parameter wins; unknown parameters are ignored (an older api never errors
/// on a newer client's question).
fn parse_list_read(raw: Option<&str>) -> Result<ListRead, String> {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(raw.unwrap_or("").as_bytes())
        .into_owned()
        .collect();
    let get = |k: &str| {
        params
            .iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    };
    if let Some(v) = get("since") {
        return match v.trim().parse::<i64>() {
            Ok(n) if n >= 0 => Ok(ListRead::Since(n)),
            _ => Err("since must be a whole number".into()),
        };
    }
    let mut q = InboxQuery::default();
    if let Some(v) = get("view") {
        q.view = InboxView::parse(v.trim()).ok_or_else(|| enum_msg(InboxView::NAMES))?;
    }
    if let Some(v) = get("unread") {
        q.unread_only = match v.trim() {
            "1" | "true" => true,
            "0" | "false" | "" => false,
            _ => return Err("unread must be 1 or 0".into()),
        };
    }
    if let Some(list) = get("category").and_then(list_param) {
        let mut cats = Vec::with_capacity(list.len());
        for c in &list {
            cats.push(parse_category(c).ok_or("unknown category")?);
        }
        q.categories = Some(cats);
    }
    if let Some(list) = get("class").and_then(list_param) {
        if !list.iter().all(|c| class_def(c).is_some()) {
            return Err("unknown notification class".into());
        }
        q.classes = Some(list);
    }
    if let Some(list) = get("kind").and_then(list_param) {
        if list.len() > KIND_LIST_MAX || list.iter().any(|k| k.chars().count() > KIND_MAX) {
            return Err(format!(
                "kind must be at most {KIND_LIST_MAX} kinds of at most {KIND_MAX} characters"
            ));
        }
        q.kinds = Some(list);
    }
    if let Some(v) = get("limit") {
        q.limit = match v.trim().parse::<i64>() {
            Ok(n) if (1..=PAGE_MAX).contains(&n) => n,
            _ => return Err(format!("limit must be a whole number from 1 to {PAGE_MAX}")),
        };
    }
    if let Some(v) = get("before").filter(|v| !v.trim().is_empty()) {
        q.before = Some(decode_cursor(v).ok_or("invalid cursor")?);
    }
    Ok(ListRead::Page(q))
}

// doc: Optional query: `view=foryou|all|done`, `unread=1`, `category=`, `class=`, `kind=` (comma lists), `limit=1..100`, `before=<next>`; or `since=<seq>` (rows with a higher seq, in seq order, ≤ 200, with `more`). None is wave 1's newest 50. See docs/notifications/live-and-inbox.md.
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let read = match parse_list_read(raw.as_deref()) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The rows and the notify seq in ONE statement (I-10): every row at or
    // below `seq` had committed when the rows were read. A page is not a
    // read THROUGH it — only `?since` with `more: false` is (I-20).
    let (notifications, seq, next, more) = match read {
        ListRead::Page(q) => match list_page(&state.pg, &user.id, &q).await {
            Ok((rows, seq, next)) => {
                let more = next.is_some();
                (rows, seq, next.as_ref().map(encode_cursor), more)
            }
            Err(e) => return Ok(internal("[notifications] list failed", e)),
        },
        ListRead::Since(since) => match list_since(&state.pg, &user.id, since).await {
            Ok((rows, seq, more)) => (rows, seq, None, more),
            Err(e) => return Ok(internal("[notifications] since read failed", e)),
        },
    };
    // Both counts from one read of the unread rows, Done ones left out of
    // both, so `unread` and `unreadForYou` can never disagree (N3) — and they
    // are the numbers the read-sync event carries.
    let (unread, unread_for_you) = match unread_counts(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[notifications] unread count failed", e)),
    };
    // Every settings key from one row, so they can never be read a moment
    // apart — and the response keeps this key order: wave 1's, then the
    // contract's, then the inbox API's.
    let is_admin = user.role == "admin";
    let (blob, zone) = prefs_and_zone(&state.pg, &user.id).await;
    let s = settings_wire(&blob, &zone, wall_ms(), is_admin);
    let email_enabled = delivery_or_off(&state.pg).await;
    Ok(Json(json!({
        "notifications": notifications,
        "unread": unread,
        "prefs": s.prefs,
        "digest": s.digest,
        "delivery": { "emailEnabled": email_enabled },
        "canSetDelivery": is_admin,
        "prompt": s.prompt,
        "unreadForYou": unread_for_you,
        "seq": seq,
        "classes": classes_wire(is_admin),
        "channels": s.channels,
        "dnd": s.dnd,
        "emailFrequency": s.email_frequency,
        "features": features(),
        "next": next,
        "more": more,
    }))
    .into_response())
}

/// What a PUT does to the selected rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PutAction {
    Read,
    Archive,
    Unarchive,
    Unread,
}

/// The `entity` member: `{type, id}`, the type one of `ENTITY_TYPES`.
fn entity_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<NotifyEntity>, String> {
    let Some(v) = obj.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let e = v.as_object().ok_or_else(|| object_msg(zod_type_name(v)))?;
    let kind = enum_member(e, "type", ENTITY_TYPES)?;
    let id = string_member(e, "id", 1, 200)?;
    Ok(Some(NotifyEntity::new(
        entity_type(&kind).expect("the enum checked it"),
        id,
    )))
}

/// The validated PUT body: the selector (ids, then href, then entity; none
/// or empty ones fold into all) and at most one action.
fn validate_put(obj: &serde_json::Map<String, Value>) -> Result<(Selector, PutAction), String> {
    // These rows. Absent or empty, with no other selector: every row of
    // yours (wave 1's mark-all).
    let ids = optional_uuid_array_member(obj, "ids", 200)?;
    // Every row of yours that points at this place.
    let href = optional_max_string_member(obj, "href", 512)?;
    // {type, id}: every row of yours about that thing (never anyone else's).
    let entity = entity_member(obj, "entity")?;
    // At most one action, each optional; none is mark-read. Done (archive)
    // also marks read; unarchive and unread need a selector.
    let flags = [
        (obj.get("archive").is_some(), PutAction::Archive),
        (obj.get("unarchive").is_some(), PutAction::Unarchive),
        (obj.get("unread").is_some(), PutAction::Unread),
    ];
    if flags[0].0 {
        // Optional. Done: archive, and mark read.
        literal_true_member(obj, "archive")?;
    }
    if flags[1].0 {
        // Optional. Back from Done (stays read); needs a selector.
        literal_true_member(obj, "unarchive")?;
    }
    if flags[2].0 {
        // Optional. Unread again (not Done rows); needs a selector.
        literal_true_member(obj, "unread")?;
    }
    let mut action = PutAction::Read;
    for (present, a) in flags {
        if present {
            if action != PutAction::Read {
                return Err("pick one of archive, unarchive or unread".into());
            }
            action = a;
        }
    }
    let sel = match (ids, href, entity) {
        (Some(ids), _, _) if !ids.is_empty() => Selector::Ids(ids),
        (_, Some(h), _) if !h.is_empty() => Selector::Href(h),
        (_, _, Some(e)) => Selector::Entity(e),
        _ => Selector::All,
    };
    if sel == Selector::All && matches!(action, PutAction::Unarchive | PutAction::Unread) {
        return Err(if action == PutAction::Unread {
            "unread needs ids, href or entity".into()
        } else {
            "unarchive needs ids, href or entity".into()
        });
    }
    Ok((sel, action))
}

// doc: A selector (`ids`, `href` or `entity`, in that precedence; none is every row) and at most one action (`archive`, `unarchive`, `unread`; none marks read). Publishes the read-sync event.
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let (sel, action) = match validate_put(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let all = sel == Selector::All;
    // (rows read, archived, made unread, back from Done), and whether this
    // path already told the other tabs (the entity clear publishes itself).
    let mut counts = (0usize, 0usize, 0usize, 0usize);
    let written = match action {
        PutAction::Read => {
            let result = match &sel {
                Selector::Entity(e) => clear_entity(&notify, &user.id, e).await,
                Selector::Ids(ids) => {
                    mark_notifications_read(&state.pg, &user.id, Some(ids.as_slice()), None).await
                }
                Selector::Href(h) => {
                    mark_notifications_read(&state.pg, &user.id, None, Some(h.as_str())).await
                }
                Selector::All => mark_notifications_read(&state.pg, &user.id, None, None).await,
            };
            result.map(|ids| {
                counts.0 = ids.len();
                (!matches!(sel, Selector::Entity(_)) && !ids.is_empty()).then_some((ids, true))
            })
        }
        // Done counts as read for the event: every client closes the banners
        // it posted for these rows, and drops them from its views.
        PutAction::Archive => {
            archive_notifications(&state.pg, &user.id, &sel)
                .await
                .map(|(archived, newly_read)| {
                    counts.0 = newly_read.len();
                    counts.1 = archived.len();
                    (!archived.is_empty()).then_some((archived, true))
                })
        }
        PutAction::Unarchive => unarchive_notifications(&state.pg, &user.id, &sel)
            .await
            .map(|ids| {
                counts.3 = ids.len();
                (!ids.is_empty()).then_some((ids, false))
            }),
        PutAction::Unread => mark_notifications_unread(&state.pg, &user.id, &sel)
            .await
            .map(|ids| {
                counts.2 = ids.len();
                (!ids.is_empty()).then_some((ids, false))
            }),
    };
    let written = match written {
        Ok(w) => w,
        Err(e) => return Ok(internal("[notifications] the inbox write failed", e)),
    };
    // The brief nudge, fired after the update — the FULL one: clear the
    // sweep throttle AND ring the bell. Awaiting it costs one UPDATE; its
    // failure is logged, never the caller's.
    if let Err(e) = mark_brief_stale(&notify, std::slice::from_ref(&user.id)).await {
        tracing::error!("[notifications] brief nudge failed: {e}");
    }
    // READ-SYNC: the person's other tabs and devices drop their count now,
    // not on their next poll, and close the banners for exactly these rows.
    // Only when something changed.
    match written {
        Some((ids, true)) => publish_read(&notify, &user.id, &ids, all).await,
        Some((_, false)) => publish_unread(&notify, &user.id).await,
        None => {}
    }
    Ok(Json(json!({
        "ok": true,
        "read": counts.0,
        "archived": counts.1,
        "unread": counts.2,
        "unarchived": counts.3,
    }))
    .into_response())
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let patch = match validate_prefs_patch(obj) {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // The master switch decides whether the whole instance mails ANYBODY.
    // Gated here (after the 400s, before any write) so one PATCH can never
    // half-apply: a member who sends both gets 403 and neither change.
    if let Some(email_enabled) = patch.delivery {
        if user.role != "admin" {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        if let Err(e) = set_notify_delivery(&state.pg, email_enabled).await {
            return Ok(internal("[notifications] set delivery failed", e));
        }
        // Audited: turning this on starts mailing every user in the
        // workspace, and "who did that, and when" is the first question.
        log_audit(
            &state.pg,
            AuditEntry {
                actor: &actor_of(&user),
                action: if email_enabled {
                    "notifications.email.enabled"
                } else {
                    "notifications.email.disabled"
                },
                target_type: "notifications",
                target_id: None,
                target_label: None,
                before: None,
                after: Some(json!({ "emailEnabled": email_enabled })),
            },
        )
        .await;
    }

    // The soft ask's answer rides its own writer: it is the person's own
    // memory, never admin-gated, and it never touches a route.
    if let Some(action) = patch.prompt
        && let Err(e) = set_notify_prompt(&state.pg, &user.id, action).await
    {
        return Ok(internal("[notifications] set prompt failed", e));
    }
    // Routes, the matrix, the digest, DND and the frequency in ONE write: a
    // legacy route and a channel bool for the same class in one PATCH apply in
    // that order, and both directions stay in step (I-13).
    if patch.prefs.is_some()
        || patch.digest.is_some()
        || patch.channels.is_some()
        || patch.dnd.is_some()
        || patch.email_frequency.is_some()
    {
        let write = PrefsWrite {
            prefs: patch.prefs,
            digest: patch.digest,
            channels: patch.channels,
            dnd: patch.dnd,
            email_frequency: patch.email_frequency,
        };
        // Includes the no-row corner — a user with no row 500s here.
        if let Err(e) = apply_notify_prefs(&state.pg, &user.id, &write).await {
            return Ok(internal("[notifications] set settings failed", e));
        }
    }
    // The answer is what a reload would GET: one read, after every write.
    let is_admin = user.role == "admin";
    let (blob, zone) = prefs_and_zone(&state.pg, &user.id).await;
    let s = settings_wire(&blob, &zone, wall_ms(), is_admin);
    let email_enabled = delivery_or_off(&state.pg).await;
    Ok(Json(json!({
        "prefs": s.prefs,
        "digest": s.digest,
        "delivery": { "emailEnabled": email_enabled },
        "canSetDelivery": is_admin,
        "prompt": s.prompt,
        "channels": s.channels,
        "dnd": s.dnd,
        "emailFrequency": s.email_frequency,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use talaria_notify::Category;

    fn patch(v: Value) -> Result<PrefsPatch, String> {
        validate_prefs_patch(v.as_object().unwrap())
    }

    #[test]
    fn prefs_patch_matches_the_zod_probe_table() {
        // The record's type message, one spelling per received type.
        let cases = [
            (json!([]), "array"),
            (json!(null), "null"),
            (json!("s"), "string"),
            (json!(5), "number"),
        ];
        for (bad, received) in cases {
            assert_eq!(
                patch(json!({ "prefs": bad })).unwrap_err(),
                format!("Invalid input: expected record, received {received}")
            );
        }
        assert_eq!(
            patch(json!({ "prefs": [] })).unwrap_err(),
            "Invalid input: expected record, received array"
        );
        // Key past 40: the generic record-key message, not the string-max.
        let long_key = "k".repeat(41);
        assert_eq!(
            patch(json!({ "prefs": { &long_key: "both" } })).unwrap_err(),
            "Invalid key in record"
        );
        // A 40-char key is fine when it names a class it can't — the class
        // refine, not the key bound, answers.
        let ok_len_bogus = "k".repeat(40);
        assert_eq!(
            patch(json!({ "prefs": { &ok_len_bogus: "both" } })).unwrap_err(),
            "unknown notification class"
        );
        // Value: anything not exactly a route is the enum message, type
        // included.
        for v in [json!(5), json!(null), json!("nope"), json!(true)] {
            assert_eq!(
                patch(json!({ "prefs": { "mention": v } })).unwrap_err(),
                "Invalid option: expected one of \"in_app\"|\"email\"|\"both\""
            );
        }
        // The field's checks, in order: empty before unknown, both before
        // the body-wide at-least-one.
        assert_eq!(
            patch(json!({ "prefs": {} })).unwrap_err(),
            "nothing to update"
        );
        assert_eq!(
            patch(json!({ "prefs": {}, "digest": "on" })).unwrap_err(),
            "nothing to update"
        );
        assert_eq!(
            patch(json!({ "prefs": { "bogus": "both" } })).unwrap_err(),
            "unknown notification class"
        );
        // Mixed good+bogus keys: the class refine still answers.
        assert_eq!(
            patch(json!({ "prefs": { "mention": "email", "bogus": "both" } })).unwrap_err(),
            "unknown notification class"
        );
        // A valid one, in every route.
        for r in ["in_app", "email", "both"] {
            let p = patch(json!({ "prefs": { "mention": r } })).unwrap();
            assert_eq!(p.prefs.as_ref().unwrap()["mention"], json!(r));
        }
        // digest: absent ok, on/off ok, anything else the enum message —
        // wrong type included.
        assert!(patch(json!({ "digest": "on" })).unwrap().digest.is_some());
        assert!(patch(json!({ "digest": "off" })).unwrap().digest.is_some());
        assert_eq!(
            patch(json!({ "digest": "nope" })).unwrap_err(),
            "Invalid option: expected one of \"on\"|\"off\""
        );
        assert_eq!(
            patch(json!({ "digest": 5 })).unwrap_err(),
            "Invalid option: expected one of \"on\"|\"off\""
        );
        // delivery: an object with a REQUIRED boolean; unknown keys strip.
        assert_eq!(
            patch(json!({ "delivery": {} })).unwrap_err(),
            "Invalid input: expected boolean, received undefined"
        );
        assert_eq!(
            patch(json!({ "delivery": { "emailEnabled": "yes" } })).unwrap_err(),
            "Invalid input: expected boolean, received string"
        );
        assert_eq!(
            patch(json!({ "delivery": [] })).unwrap_err(),
            "Invalid input: expected object, received array"
        );
        assert_eq!(
            patch(json!({ "delivery": { "emailEnabled": true, "junk": 1 } }))
                .unwrap()
                .delivery,
            Some(true)
        );
        // The at-least-one check: nothing present (or only stripped
        // unknowns) is 'nothing to update', and it runs LAST — a bad prefs
        // beside an otherwise-empty body answers prefs' message.
        // prompt: exactly one answer, as a literal true.
        assert_eq!(
            patch(json!({ "prompt": { "dismissed": true } }))
                .unwrap()
                .prompt,
            Some(PromptAction::Dismissed)
        );
        assert_eq!(
            patch(json!({ "prompt": { "granted": true } }))
                .unwrap()
                .prompt,
            Some(PromptAction::Granted)
        );
        for bad in [
            json!({}),
            json!({ "dismissed": false }),
            json!({ "dismissed": "yes" }),
            json!({ "dismissed": true, "granted": true }),
        ] {
            assert_eq!(
                patch(json!({ "prompt": bad })).unwrap_err(),
                "prompt must be {dismissed: true} or {granted: true}"
            );
        }
        assert_eq!(
            patch(json!({ "prompt": [] })).unwrap_err(),
            "Invalid input: expected object, received array"
        );
        assert_eq!(patch(json!({})).unwrap_err(), "nothing to update");
        assert_eq!(
            patch(json!({ "bogus": "x" })).unwrap_err(),
            "nothing to update"
        );
        assert_eq!(
            patch(json!({ "prefs": 5 })).unwrap_err(),
            "Invalid input: expected record, received number"
        );
    }

    fn page(raw: &str) -> InboxQuery {
        match parse_list_read(Some(raw)).unwrap() {
            ListRead::Page(q) => q,
            other => panic!("expected a page, got {other:?}"),
        }
    }

    #[test]
    fn the_list_parameters_are_pinned() {
        // None: wave 1's newest 50, every row.
        assert_eq!(
            parse_list_read(None).unwrap(),
            ListRead::Page(InboxQuery::default())
        );
        assert_eq!(page(""), InboxQuery::default());
        let q = page(
            "view=foryou&unread=1&category=direct,needs_you&class=dm&kind=dm,mention&limit=20",
        );
        assert_eq!(q.view, InboxView::ForYou);
        assert!(q.unread_only);
        assert_eq!(
            q.categories.as_deref(),
            Some(&[Category::Direct, Category::NeedsYou][..])
        );
        assert_eq!(q.classes.as_deref(), Some(&["dm".to_string()][..]));
        assert_eq!(q.kinds.as_ref().unwrap().len(), 2);
        assert_eq!(q.limit, 20);
        assert_eq!(page("view=done").view, InboxView::Done);
        assert_eq!(page("view=all&unread=0").unread_only, false);
        // An empty list is no filter; unknown parameters are ignored.
        assert_eq!(page("category=&class=,&future=1"), InboxQuery::default());
        // `since` wins, and the view parameters are not even read.
        assert_eq!(
            parse_list_read(Some("since=12&view=bogus")).unwrap(),
            ListRead::Since(12)
        );
        let err = |raw: &str| parse_list_read(Some(raw)).unwrap_err();
        assert_eq!(err("since=-1"), "since must be a whole number");
        assert_eq!(err("since=x"), "since must be a whole number");
        assert_eq!(
            err("view=inbox"),
            "Invalid option: expected one of \"foryou\"|\"all\"|\"done\""
        );
        assert_eq!(err("unread=yes"), "unread must be 1 or 0");
        assert_eq!(err("category=direct,loud"), "unknown category");
        assert_eq!(err("class=dm,bogus"), "unknown notification class");
        assert_eq!(err("limit=0"), "limit must be a whole number from 1 to 100");
        assert_eq!(
            err("limit=101"),
            "limit must be a whole number from 1 to 100"
        );
        assert_eq!(err("before=not-a-cursor"), "invalid cursor");
        let many: Vec<String> = (0..51).map(|i| format!("k{i}")).collect();
        assert_eq!(
            err(&format!("kind={}", many.join(","))),
            "kind must be at most 50 kinds of at most 64 characters"
        );
    }

    #[test]
    fn the_cursor_round_trips_and_refuses_anything_else() {
        let c = Cursor {
            at_us: 1_730_419_200_123_456,
            id: "5e000000-0000-4000-8000-00000000000a".into(),
        };
        let wire = encode_cursor(&c);
        assert!(!wire.contains('='), "url-safe, unpadded: {wire}");
        assert_eq!(decode_cursor(&wire), Some(c.clone()));
        assert_eq!(page(&format!("before={wire}")).before, Some(c));
        assert_eq!(
            decode_cursor(&URL_SAFE_NO_PAD.encode("12.not-a-uuid")),
            None
        );
        assert_eq!(
            decode_cursor(&URL_SAFE_NO_PAD.encode("x.5e000000-0000-4000-8000-00000000000a")),
            None
        );
        assert_eq!(decode_cursor("%%%"), None);
    }

    fn put(v: Value) -> Result<(Selector, PutAction), String> {
        validate_put(v.as_object().unwrap())
    }

    #[test]
    fn the_put_members_are_pinned() {
        let id = "5e000000-0000-4000-8000-00000000000a";
        // Wave 1's bodies, unchanged: ids, href, all (and empty ones fold).
        assert_eq!(put(json!({})).unwrap(), (Selector::All, PutAction::Read));
        assert_eq!(
            put(json!({ "ids": [id] })).unwrap(),
            (Selector::Ids(vec![id.into()]), PutAction::Read)
        );
        assert_eq!(
            put(json!({ "ids": [], "href": "/plan/a" })).unwrap().0,
            Selector::Href("/plan/a".into())
        );
        assert_eq!(
            put(json!({ "ids": [], "href": "" })).unwrap().0,
            Selector::All
        );
        // ids win over href, href over entity.
        assert_eq!(
            put(json!({ "ids": [id], "href": "/x", "entity": { "type": "task", "id": "t1" } }))
                .unwrap()
                .0,
            Selector::Ids(vec![id.into()])
        );
        assert_eq!(
            put(json!({ "entity": { "type": "task", "id": "t1" } })).unwrap(),
            (
                Selector::Entity(NotifyEntity::new("task", "t1")),
                PutAction::Read
            )
        );
        // The actions: literal true, at most one.
        assert_eq!(
            put(json!({ "ids": [id], "archive": true })).unwrap().1,
            PutAction::Archive
        );
        assert_eq!(
            put(json!({ "archive": true })).unwrap(),
            (Selector::All, PutAction::Archive)
        );
        assert_eq!(
            put(json!({ "ids": [id], "unarchive": true })).unwrap().1,
            PutAction::Unarchive
        );
        assert_eq!(
            put(json!({ "href": "/x", "unread": true })).unwrap().1,
            PutAction::Unread
        );
        assert_eq!(
            put(json!({ "ids": [id], "archive": false })).unwrap_err(),
            "Invalid input: expected true"
        );
        assert_eq!(
            put(json!({ "ids": [id], "archive": true, "unread": true })).unwrap_err(),
            "pick one of archive, unarchive or unread"
        );
        // Unread and unarchive need a selector: no "resurrect everything".
        assert_eq!(
            put(json!({ "unread": true })).unwrap_err(),
            "unread needs ids, href or entity"
        );
        assert_eq!(
            put(json!({ "ids": [], "unarchive": true })).unwrap_err(),
            "unarchive needs ids, href or entity"
        );
        // The entity selector's own checks.
        assert_eq!(
            put(json!({ "entity": { "type": "secret", "id": "x" } })).unwrap_err(),
            enum_msg(ENTITY_TYPES)
        );
        assert_eq!(
            put(json!({ "entity": { "type": "task" } })).unwrap_err(),
            "Invalid input: expected string, received undefined"
        );
        assert_eq!(
            put(json!({ "entity": "task:1" })).unwrap_err(),
            "Invalid input: expected object, received string"
        );
        assert_eq!(put(json!({ "ids": ["nope"] })).unwrap_err(), "Invalid UUID");
    }

    #[test]
    fn the_v2_members_are_pinned() {
        // channels: a record of class → bools; in-app is not a choice.
        let p = patch(json!({ "channels": { "mention": { "email": false } } })).unwrap();
        assert_eq!(
            p.channels.unwrap(),
            vec![(
                "mention".to_string(),
                ChannelsPatch {
                    email: Some(false),
                    ..Default::default()
                }
            )]
        );
        assert_eq!(
            patch(json!({ "channels": { "mention": { "inApp": false } } })).unwrap_err(),
            "In-app is always on"
        );
        assert!(patch(json!({ "channels": { "dm": { "inApp": true, "push": false } } })).is_ok());
        assert_eq!(
            patch(json!({ "channels": { "bogus": { "email": true } } })).unwrap_err(),
            "unknown notification class"
        );
        // The new classes are writable through channels (never through prefs).
        assert!(patch(json!({ "channels": { "participating": { "email": true } } })).is_ok());
        assert_eq!(
            patch(json!({ "prefs": { "participating": "email" } })).unwrap_err(),
            "unknown notification class"
        );
        assert_eq!(
            patch(json!({ "channels": { "mention": { "email": "no" } } })).unwrap_err(),
            "Invalid input: expected boolean, received string"
        );
        assert_eq!(
            patch(json!({ "channels": [] })).unwrap_err(),
            "Invalid input: expected record, received array"
        );
        assert_eq!(
            patch(json!({ "channels": { "mention": true } })).unwrap_err(),
            "Invalid input: expected object, received boolean"
        );
        assert_eq!(
            patch(json!({ "channels": {} })).unwrap_err(),
            "nothing to update"
        );
        // dnd: a pause up to FOREVER, working hours as weekdays and HH:MM.
        assert!(patch(json!({ "dnd": { "pausedUntil": "2026-11-01T12:00:00.000Z" } })).is_ok());
        assert!(patch(json!({ "dnd": { "pausedUntil": "9999-12-31T23:59:59.999Z" } })).is_ok());
        assert!(patch(json!({ "dnd": { "pausedUntil": null } })).is_ok());
        assert_eq!(
            patch(json!({ "dnd": { "pausedUntil": "tomorrow" } })).unwrap_err(),
            "pausedUntil must be an ISO time, or null"
        );
        let sched = |start: &str, end: &str, days: Value| {
            patch(
                json!({ "dnd": { "schedule": { "enabled": true, "days": days, "start": start, "end": end } } }),
            )
        };
        let ok = sched("09:00", "17:30", json!([5, 1, 3])).unwrap();
        assert_eq!(
            ok.dnd.unwrap().schedule.unwrap().unwrap().days,
            vec![1, 3, 5]
        );
        assert_eq!(
            sched("9:00", "17:00", json!([1])).unwrap_err(),
            "start must be HH:MM"
        );
        assert_eq!(
            sched("09:00", "24:00", json!([1])).unwrap_err(),
            "end must be HH:MM"
        );
        assert_eq!(
            sched("09:00", "17:00", json!([7])).unwrap_err(),
            "days must be weekday numbers 0-6"
        );
        assert_eq!(
            sched("09:00", "17:00", json!([1, 1])).unwrap_err(),
            "days must be weekday numbers 0-6"
        );
        assert_eq!(
            patch(
                json!({ "dnd": { "schedule": { "days": [1], "start": "09:00", "end": "17:00" } } })
            )
            .unwrap_err(),
            "Invalid input: expected boolean, received undefined"
        );
        assert!(patch(json!({ "dnd": { "schedule": null } })).is_ok());
        assert_eq!(
            patch(json!({ "dnd": { "allowNeedsYou": "y" } })).unwrap_err(),
            "Invalid input: expected boolean, received string"
        );
        assert_eq!(
            patch(json!({ "dnd": {} })).unwrap_err(),
            "nothing to update"
        );
        assert_eq!(
            patch(json!({ "dnd": [] })).unwrap_err(),
            "Invalid input: expected object, received array"
        );
        // emailFrequency: the enum.
        assert_eq!(
            patch(json!({ "emailFrequency": "1h" }))
                .unwrap()
                .email_frequency
                .as_deref(),
            Some("1h")
        );
        assert_eq!(
            patch(json!({ "emailFrequency": "2h" })).unwrap_err(),
            "Invalid option: expected one of \"15m\"|\"1h\"|\"1d\"|\"off\""
        );
    }
}
