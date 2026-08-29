// /api/notifications — port of ui/src/routes/api/notifications.ts. The
// caller's inbox: GET is the bell's one read (list, unread, prefs, digest,
// the instance switch, whether THIS user may flip it), PUT marks read, PATCH
// changes routing. The mail fan-out behind the prefs (sendGatedMail, the
// outbox, the drain) is scheduler plane — batch 5 — so it is not here; this
// file ends at the rows and the switches the SPA reads and writes.
//
// The admin gate is a role check on an ALREADY-required user, exactly where
// TS puts it: after the body's 400s, before any write, so a member who sends
// prefs and delivery in one PATCH gets 403 and NEITHER change — one PATCH can
// never half-apply.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{
    as_object, boolean_member, enum_member, enum_msg, object_msg, optional_uuid_array_member,
    parse, record_msg, utf16_len, zod_type_name,
};
use crate::error::house_error;
use crate::notify::{
    NOTIFY_CLASSES, get_notify_delivery, get_notify_settings, list_notifications,
    mark_notifications_read, nudge_brief, set_notify_delivery, set_notify_settings, unread_count,
};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

const ROUTE_OPTS: &[&str] = &["in_app", "email", "both"];
const DIGEST_OPTS: &[&str] = &["on", "off"];

/// The validated PrefsPatch: each field Option<"present">, which is the very
/// thing the root refine counts.
#[derive(Debug)]
struct PrefsPatch {
    prefs: Option<serde_json::Map<String, Value>>,
    digest: Option<String>,
    delivery: Option<bool>,
}

/// notifications.ts's PrefsPatch, checks in zod's schema order: prefs (the
/// record, then its two refines), digest, delivery — then the root refine.
/// Every message is probed against the ui's zod 4.3.6 and pinned in the test
/// at the bottom of this file.
fn validate_prefs_patch(obj: &serde_json::Map<String, Value>) -> Result<PrefsPatch, String> {
    let prefs = match obj.get("prefs") {
        None => None,
        Some(v) => {
            // z.record(z.string().max(40), ROUTE): entry by entry in document
            // order, key then value. A key past 40 chars is the GENERIC
            // record-key message (not the string-max spelling), and a value
            // that is not exactly a route is the enum's message whatever its
            // JSON type.
            let m = v.as_object().ok_or_else(|| record_msg(zod_type_name(v)))?;
            for (k, val) in m {
                if utf16_len(k) > 40 {
                    return Err("Invalid key in record".into());
                }
                if !val.as_str().is_some_and(|s| ROUTE_OPTS.contains(&s)) {
                    return Err(enum_msg(ROUTE_OPTS));
                }
            }
            // .refine(non-empty) then .refine(all-known) — the field's own
            // refines outrank the root's, so `{}` with a valid digest beside
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
    if prefs.is_none() && digest.is_none() && delivery.is_none() {
        return Err("nothing to update".into());
    }
    Ok(PrefsPatch {
        prefs,
        digest,
        delivery,
    })
}

/// deliveryOrOff: the switch read that must not fail the read it rides on —
/// GET here answers "what is in your inbox", and the honest answer for a
/// switch that defaults to off is off. get_notify_delivery already falls
/// back on a failed settings read, so this is infallible by construction.
async fn delivery_or_off(pg: &sqlx::PgPool) -> bool {
    get_notify_delivery(pg).await
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let notifications = match list_notifications(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[notifications] list failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let unread = match unread_count(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[notifications] unread count failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    // prefs + digest from one row, so the two can never be read a moment
    // apart (getNotifySettings); the spread lands them here in TS's order.
    let (prefs, digest) = get_notify_settings(&state.pg, &user.id).await;
    let email_enabled = delivery_or_off(&state.pg).await;
    Json(json!({
        "notifications": notifications,
        "unread": unread,
        "prefs": prefs,
        "digest": digest,
        "delivery": { "emailEnabled": email_enabled },
        "canSetDelivery": user.role == "admin",
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // ids absent OR EMPTY both mean "all of mine" — TS's own ids.length > 0
    // guard folds them together, and so does the data layer.
    let ids = match optional_uuid_array_member(obj, "ids", 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = mark_notifications_read(&state.pg, &user.id, ids.as_deref()).await {
        tracing::error!("[notifications] mark-read failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    // The brief nudge TS fires detached after the update; awaiting it changes
    // nothing on the wire and its failures are swallowed inside.
    nudge_brief(&state.pg, &user.id).await;
    Json(json!({ "ok": true })).into_response()
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let patch = match validate_prefs_patch(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };

    // The master switch decides whether the whole instance mails ANYBODY.
    // Gated here (after the 400s, before any write) so one PATCH can never
    // half-apply: a member who sends both gets 403 and neither change.
    if let Some(email_enabled) = patch.delivery {
        if user.role != "admin" {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        if let Err(e) = set_notify_delivery(&state.pg, email_enabled).await {
            tracing::error!("[notifications] set delivery failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
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

    let (prefs, digest) = if patch.prefs.is_some() || patch.digest.is_some() {
        match set_notify_settings(
            &state.pg,
            &user.id,
            patch.prefs.as_ref(),
            patch.digest.as_deref(),
        )
        .await
        {
            Ok(v) => v,
            // Includes the no-row corner ('no such user' on TS) — the
            // recorded platform-500 divergence.
            Err(e) => {
                tracing::error!("[notifications] set settings failed: {e}");
                return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
            }
        }
    } else {
        get_notify_settings(&state.pg, &user.id).await
    };
    let email_enabled = delivery_or_off(&state.pg).await;
    Json(json!({
        "prefs": prefs,
        "digest": digest,
        "delivery": { "emailEnabled": email_enabled },
        "canSetDelivery": user.role == "admin",
    }))
    .into_response()
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
        // Every row probed against PrefsPatch in the ui's zod 4.3.6.
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
        // The field's refines, in order: empty before unknown-before-root.
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
        // The root refine: nothing present (or only stripped unknowns) is
        // 'nothing to update', and it runs LAST — a bad prefs beside an
        // otherwise-empty body answers prefs' message.
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
}
