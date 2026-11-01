// The per-user routing table in `users.notify_prefs`, the digest's derived
// answer, the soft ask's memory under the reserved `prompt` key, and the
// instance email master switch in app_settings.

use serde_json::Value;
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_settings::{get_setting, set_setting};

use crate::classes::NOTIFY_CLASSES;

const DIGEST_PREF_KEY: &str = "digest";
pub(crate) const DELIVERY_KEY: &str = "notify_delivery";
const ROUTES: &[&str] = &["in_app", "email", "both"];

fn is_route(v: &Value) -> bool {
    v.as_str().is_some_and(|s| ROUTES.contains(&s))
}

/// The full routing table: stored choice when it is a route we understand,
/// the class fallback otherwise.
/// Key order is NOTIFY_CLASSES order.
pub fn resolve_prefs(stored: &Value) -> Value {
    let mut m = serde_json::Map::new();
    for (id, fallback) in NOTIFY_CLASSES {
        let stored_route = stored
            .get(*id)
            .and_then(|v| v.as_str())
            .filter(|s| ROUTES.contains(s));
        m.insert(
            (*id).to_string(),
            Value::String(stored_route.unwrap_or(fallback).to_string()),
        );
    }
    Value::Object(m)
}

/// The effective digest answer: an explicit on/off wins outright; with no
/// explicit choice it is DERIVED in the direction that cannot spam — someone
/// who routed every class in-app is not mailed a digest either.
pub fn digest_enabled(stored: &Value) -> bool {
    if let Some(explicit) = stored.get(DIGEST_PREF_KEY).and_then(|v| v.as_str())
        && matches!(explicit, "on" | "off")
    {
        return explicit == "on";
    }
    let table = resolve_prefs(stored);
    NOTIFY_CLASSES
        .iter()
        .any(|(id, _)| table.get(*id).and_then(|v| v.as_str()) != Some("in_app"))
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

/// Everything the Settings panel shows, from one row: the resolved table and
/// the effective digest answer. Never fails.
pub async fn get_notify_settings(pg: &PgPool, user_id: &str) -> (Value, &'static str) {
    let blob = prefs_blob(pg, user_id).await.unwrap_or(Value::Null);
    (
        resolve_prefs(&blob),
        if digest_enabled(&blob) { "on" } else { "off" },
    )
}

/// Merge a patch into the stored blob and return the effective settings.
/// Unknown keys are dropped rather than stored — the column is jsonb and this
/// is the writer that keeps it honest. Err (no row) is the deleted-user
/// corner.
pub async fn set_notify_settings(
    pg: &PgPool,
    user_id: &str,
    prefs: Option<&serde_json::Map<String, Value>>,
    digest: Option<&str>,
) -> Result<(Value, &'static str), sqlx::Error> {
    let mut clean = serde_json::Map::new();
    if let Some(prefs) = prefs {
        for (id, _) in NOTIFY_CLASSES {
            if let Some(v) = prefs.get(*id).filter(|v| is_route(v)) {
                clean.insert((*id).to_string(), v.clone());
            }
        }
    }
    if let Some(d) = digest
        && matches!(d, "on" | "off")
    {
        clean.insert(DIGEST_PREF_KEY.to_string(), Value::String(d.to_string()));
    }
    let row: Option<(Value,)> = sqlx::query_as(
        "update users set notify_prefs = coalesce(notify_prefs, '{}'::jsonb) || $2 \
         where id = $1::uuid returning notify_prefs",
    )
    .bind(user_id)
    .bind(Value::Object(clean))
    .fetch_optional(pg)
    .await?;
    let Some((blob,)) = row else {
        return Err(sqlx::Error::RowNotFound);
    };
    Ok((
        resolve_prefs(&blob),
        if digest_enabled(&blob) { "on" } else { "off" },
    ))
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
pub async fn set_notify_delivery(pg: &PgPool, email_enabled: bool) -> Result<(), sqlx::Error> {
    set_setting(
        pg,
        DELIVERY_KEY,
        &serde_json::json!({ "emailEnabled": email_enabled }),
    )
    .await
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
}
