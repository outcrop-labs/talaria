// /api/notifications. The caller's inbox: GET is the bell's one read (list,
// unread, prefs, digest, the instance switch, whether THIS user may flip it,
// and the desktop-alerts soft ask's memory — and, since the contract, the v2
// rows, the For-you count, the notify seq read in the same statement as the
// rows, the class registry, the channel matrix, DND, the mail frequency and
// the feature flags), PUT marks read — by ids, by href, or all of them — and
// tells the person's other tabs and devices which rows, and PATCH changes
// routing, the matrix, DND or the frequency, or records a soft-ask answer.
// The mail fan-out behind the prefs (the outbox, the drain) is scheduler
// plane, so it is not here; this file ends at the rows and the switches the
// SPA reads and writes. Every key wave 1 served is still served, in order.
//
// The admin gate is a role check on an ALREADY-required user, ordered after
// the body's 400s and before any write, so a member who sends prefs and
// delivery in one PATCH gets 403 and NEITHER change — one PATCH can never
// half-apply.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::now_ms as wall_ms;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{
    boolean_member, boolean_msg, enum_member, enum_msg, object_msg, optional_boolean_member,
    optional_max_string_member, optional_uuid_array_member, parse, record_msg, utf16_len,
    zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{
    ChannelsPatch, DndPatch, DndSchedule, EMAIL_FREQUENCIES, FOREVER_MS, NOTIFY_CLASSES,
    NotifyDeps, PrefsWrite, PromptAction, apply_notify_prefs, class_def, classes_wire, features,
    get_notify_delivery, list_inbox, mark_brief_stale, mark_notifications_read, parse_hhmm,
    parse_instant, prefs_and_zone, publish_read, set_notify_delivery, set_notify_prompt,
    settings_wire, unread_count, unread_counts,
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

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // The rows and the notify seq in ONE statement (I-10): a client that
    // records `seq` as read through has seen every row at or below it.
    let (notifications, seq) = match list_inbox(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[notifications] list failed", e)),
    };
    let unread = match unread_count(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return Ok(internal("[notifications] unread count failed", e)),
    };
    let unread_for_you = match unread_counts(&state.pg, &user.id).await {
        Ok((_, for_you)) => for_you,
        Err(e) => return Ok(internal("[notifications] unread count failed", e)),
    };
    // Every settings key from one row, so they can never be read a moment
    // apart — and the response keeps this key order: wave 1's, then the
    // contract's.
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
        "next": Value::Null,
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // ids absent OR EMPTY both mean "all of mine" — the data layer folds
    // them together. `href` is the surface's selector (mark everything that
    // points at the place I just opened); the data layer gives ids priority
    // when both arrive, the same precedence this parse checks them in.
    let ids = match optional_uuid_array_member(obj, "ids", 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let href = match optional_max_string_member(obj, "href", 512) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let read =
        match mark_notifications_read(&state.pg, &user.id, ids.as_deref(), href.as_deref()).await {
            Ok(ids) => ids,
            Err(e) => return Ok(internal("[notifications] mark-read failed", e)),
        };
    // A mark-all: no ids and no href (empty ones fold into all).
    let all = ids.as_deref().is_none_or(<[String]>::is_empty)
        && href.as_deref().is_none_or(str::is_empty);
    // The brief nudge, fired after the update — the FULL one: clear the
    // sweep throttle AND ring the bell. Awaiting it costs one UPDATE; its
    // failure is logged, never the caller's.
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = mark_brief_stale(&notify, std::slice::from_ref(&user.id)).await {
        tracing::error!("[notifications] brief nudge failed: {e}");
    }
    // READ-SYNC: the person's other tabs and devices drop their count now,
    // not on their next poll, and close the banners for exactly these rows.
    // Only when something was actually read.
    if !read.is_empty() {
        publish_read(&notify, &user.id, &read, all).await;
    }
    Ok(Json(json!({ "ok": true })).into_response())
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
