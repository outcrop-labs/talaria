// Notifications — the user's inbox half of ui/src/server/notifications.ts:
// list, unread count, mark-read, the per-user routing table in
// users.notify_prefs, and the instance email master switch in app_settings.
// The mail fan-out (sendGatedMail, the queue, the drain) is scheduler plane —
// it stays TS until batch 5, which is also why addNotification is not ported:
// its callers are TS routes.
//
// The class vocabulary is lib/notify-classes.ts ported whole: classes,
// fallbacks, the digest's reserved key. resolve/digest DERIVE the effective
// answer here exactly as there, from the same jsonb, so a hand-edited row
// degrades identically on both runtimes.

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::settings::{get_setting, set_setting};
use serde_json::Value;
use sqlx::PgPool;

/// One class as lib/notify-classes.ts declares it — id and the fallback route
/// a person who never touched the switch gets. Order is the wire order: the
/// resolved prefs object serializes in this sequence.
pub const NOTIFY_CLASSES: &[(&str, &str)] = &[
    ("mention", "both"),
    ("dm", "both"),
    ("approval_pending", "both"),
    ("judge_escalation", "both"),
    ("agent_blocked", "both"),
    ("gap_reported", "in_app"),
    ("work_complete", "in_app"),
];

const DIGEST_PREF_KEY: &str = "digest";
const DELIVERY_KEY: &str = "notify_delivery";
const ROUTES: &[&str] = &["in_app", "email", "both"];

fn is_route(v: &Value) -> bool {
    v.as_str().is_some_and(|s| ROUTES.contains(&s))
}

/// The full routing table: stored choice when it is a route we understand,
/// the class fallback otherwise (lib/notify-classes.ts resolveNotifyPrefs).
/// Key order is NOTIFY_CLASSES order — what Object.fromEntries gives there.
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
/// who routed every class in-app is not mailed a digest either
/// (lib/notify-classes.ts digestEnabled).
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

/// The notification row as the LIST serves it — the select's key order
/// (id, kind, title, body, href, readAt, createdAt). Timestamps ride as
/// epoch-ms → ISO, which is byte-identical to postgres.js handing TS a Date
/// and JSON.stringify writing toISOString.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub href: String,
    pub read_at: Option<String>,
    pub created_at: String,
}

/// One inbox row as the select hands it back, in column order — the epoch-ms
/// pair at the tail is read_at?/created_at.
type NotificationRow = (String, String, String, String, String, Option<i64>, i64);

pub async fn list_notifications(
    pg: &PgPool,
    user_id: &str,
) -> Result<Vec<Notification>, sqlx::Error> {
    let rows: Vec<NotificationRow> = sqlx::query_as(
        "select id::text, kind, title, body, href, \
                (trunc(extract(epoch from read_at) * 1000))::bigint, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from notifications where user_id = $1::uuid \
         order by created_at desc limit 50",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, kind, title, body, href, read_ms, created_ms)| Notification {
                id,
                kind,
                title,
                body,
                href,
                read_at: read_ms.map(epoch_ms_to_iso),
                created_at: epoch_ms_to_iso(created_ms),
            },
        )
        .collect())
}

pub async fn unread_count(pg: &PgPool, user_id: &str) -> Result<i32, sqlx::Error> {
    let n: (i32,) = sqlx::query_as(
        "select count(*)::int from notifications where user_id = $1::uuid and read_at is null",
    )
    .bind(user_id)
    .fetch_one(pg)
    .await?;
    Ok(n.0)
}

/// Mark specific notifications read, or all of the user's when ids is
/// omitted — and when it is EMPTY, which TS's own `ids.length > 0` guard
/// folds into the same all-rows update.
pub async fn mark_notifications_read(
    pg: &PgPool,
    user_id: &str,
    ids: Option<&[String]>,
) -> Result<(), sqlx::Error> {
    match ids {
        Some(list) if !list.is_empty() => {
            sqlx::query(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and id = any($2::uuid[]) and read_at is null",
            )
            .bind(user_id)
            .bind(list)
            .execute(pg)
            .await?;
        }
        _ => {
            sqlx::query(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and read_at is null",
            )
            .bind(user_id)
            .execute(pg)
            .await?;
        }
    }
    Ok(())
}

/// The brief-nudge half of markBriefStale: clear today's sweep throttle so
/// the person's next /api/brief re-sweeps. The realtime publish that follows
/// it on TS is SSE — batch 5's plane — so it is not written here (rule 5);
/// the degradation is exactly the one TS tolerates when the nudge's own
/// `.catch(() => {})` swallows a failure. Fire-and-forget by design there.
pub async fn nudge_brief(pg: &PgPool, user_id: &str) {
    if let Err(e) = sqlx::query(
        "update daily_briefs set last_swept_at = null \
         where user_id = $1::uuid and created_at > now() - interval '48 hours'",
    )
    .bind(user_id)
    .execute(pg)
    .await
    {
        tracing::error!("[notifications] brief nudge failed: {e}");
    }
}

/// The raw notify_prefs blob, None on any read failure — prefsBlob's
/// forgiving-by-design corner: a preference is a modifier with a perfectly
/// good default, and the inbox must not 500 over the column that only
/// decides where mail goes.
async fn prefs_blob(pg: &PgPool, user_id: &str) -> Option<Value> {
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
/// corner: TS throws 'no such user' out of the route.
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

/// Is this instance sending notification mail? Off unless the settings row
/// says exactly true — a hand-edited row must not be able to turn mail on
/// (getNotifyDelivery). get_setting already falls back on a failed read,
/// which is deliveryOrOff's catch.
pub async fn get_notify_delivery(pg: &PgPool) -> bool {
    let stored = get_setting(pg, DELIVERY_KEY, serde_json::json!({})).await;
    stored.get("emailEnabled") == Some(&Value::Bool(true))
}

/// Turn notification email on or off for the whole instance — the admin
/// master switch (setNotifyDelivery). The 10s in-process cache and the
/// console.warn on TS are not wire-visible; the DB row is the contract.
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
            "gap_reported": "in_app", "work_complete": "in_app"
        });
        assert!(!digest_enabled(&all_in_app));
        // A garbage digest value falls back to the derivation, not to off.
        assert!(digest_enabled(&json!({ "digest": "garbage" })));
    }
}
