// Notifications — the user's inbox half of ui/src/server/notifications.ts:
// list, unread count, mark-read, the per-user routing table in
// users.notify_prefs, the instance email master switch in app_settings, and
// the single writer (`add_notification`) every filing of a row goes through.
// The mail fan-out's DRAIN (sendGatedMail, the queue, the breaker) is
// scheduler plane — it crosses with the scheduler slice of this batch, and the
// mail leg of `add_notification` is documented there.
//
// The class vocabulary is lib/notify-classes.ts ported whole: classes,
// fallbacks, the digest's reserved key. resolve/digest DERIVE the effective
// answer here exactly as there, from the same jsonb, so a hand-edited row
// degrades identically on both runtimes.

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::settings::{get_setting, set_setting};
use crate::realtime::{RealtimeDeps, UserEvent, publish_user};
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

// ── The write: one notification, filed ───────────────────────────────────────

/// `kind` (what the writer called it) → the class a user has an opinion about
/// (lib/notify-classes.ts KIND_CLASS). Kinds are free-form strings at ~10 call
/// sites and predate this table; the map is how they stay free-form without
/// every new one inventing a setting.
fn kind_class(kind: &str) -> Option<&'static str> {
    Some(match kind {
        // Someone pointed at you on purpose.
        "mention" | "kb-comment" | "task-assigned" | "plan-share" | "research-share" => "mention",
        // Addressed to you, in a thread of your own.
        "dm" | "agent-outreach" => "dm",
        // Blocked on a human.
        "agent-problem" | "workbench-repo-request" => "agent_blocked",
        // Outcomes.
        "research" | "task-status" => "work_complete",
        // `judge_escalation` and `gap_reported` are NOT here on purpose: their
        // writers name the CLASS as the kind, which `notify_class_of` accepts
        // directly. A kind that is already the class it belongs to has nothing
        // to map.
        _ => return None,
    })
}

/// The class a notification belongs to (notifyClassOf). Every class id is also
/// accepted as a kind, so a new writer can name the class directly and skip
/// the table.
///
/// An UNRECOGNIZED kind lands in `work_complete` — the quiet bucket — on
/// purpose: a notification kind added by someone who never opened this file
/// must not start mailing the whole org because it fell through a default.
pub fn notify_class_of(kind: &str) -> &'static str {
    kind_class(kind).unwrap_or_else(|| {
        // The class table's OWN static id, not the caller's borrow — the ids
        // are the same bytes, and only one of them can be returned as
        // &'static.
        NOTIFY_CLASSES
            .iter()
            .find(|(id, _)| *id == kind)
            .map(|(id, _)| *id)
            .unwrap_or("work_complete")
    })
}

/// What a writer files: the kind routes through the class table; title, body
/// and href are CONTENT and reach only the person whose row this is.
pub struct NotificationInput<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub href: Option<&'a str>,
}

/// The edges a notification write touches beyond its own row: the pool the row
/// lands in and the realtime fan-out that tells the person's open pages.
#[derive(Clone)]
pub struct NotifyDeps {
    pub pg: PgPool,
    pub realtime: RealtimeDeps,
}

impl NotifyDeps {
    /// The publish half over the shared manager, from the pieces every route
    /// already holds. Redis unreachable at construction is logged inside
    /// `publish_only` and publishes nothing — the row is still the record.
    pub fn publishing(pg: PgPool, conn: Option<redis::aio::ConnectionManager>) -> Self {
        Self {
            pg,
            realtime: RealtimeDeps::publish_only(conn),
        }
    }
}

/// daily-brief-stale.ts markBriefStale, complete now that the realtime plane
/// has crossed: mark these people's current brief as needing a sweep, AND tell
/// their open pages it moved.
///
/// Bounded to the last 48 hours so a long-lived account does not have every
/// brief it has ever had rewritten by one DM. Only today's is ever swept, so
/// older rows are noise either way — the window is about the size of the
/// UPDATE, not about correctness. What it DOES NOT DO is sweep: a sweep is
/// four scoped queries and up to one model call; this clears the throttle and
/// rings the bell, and the next read does the work.
pub async fn mark_brief_stale(deps: &NotifyDeps, user_ids: &[String]) -> Result<(), sqlx::Error> {
    // TS: [...new Set(userIds)].filter(Boolean) — deduped, and an empty id is
    // nobody's brief, not a row to look up.
    let mut ids: Vec<String> = user_ids.to_vec();
    ids.sort();
    ids.dedup();
    ids.retain(|id| !id.is_empty());
    if ids.is_empty() {
        return Ok(());
    }
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "update daily_briefs set last_swept_at = null \
         where user_id = any($1::uuid[]) and created_at > now() - interval '48 hours' \
         returning id::text, user_id::text, last_seq",
    )
    .bind(&ids)
    .fetch_all(&deps.pg)
    .await?;
    // Id-shaped, like every other user event: the payload says a brief moved
    // and the client re-reads through the ordinary route with the ordinary ACL.
    for (brief_id, user_id, seq) in rows {
        publish_user(
            &deps.realtime,
            &user_id,
            &UserEvent::Brief { brief_id, seq },
        );
    }
    Ok(())
}

/// notifications.ts addNotification — THE single writer. Ten call sites file
/// rows; this one decides where they go. The caller's contract is fire and
/// forget: the row is the record, and everything after it is a delivery that
/// must not cost the request it rode in on.
///
/// THE ROW LANDS UNREAD, ALWAYS. `email` files it read afterwards (in the
/// drain, once a provider has accepted the mail) so a class you read in your
/// mail does not also nag you with an unread count — but "afterwards" is doing
/// all the work in that sentence. Everything between the intention to send and
/// an inbox is allowed to fail, and every one of those failures would leave
/// the person with no email AND a notification that was born read: not in the
/// bell count, not in the unread list, findable only by scrolling an inbox
/// they have no reason to open. A delivery that did not happen must not mark
/// the record consumed.
///
/// THE MAIL LEG WAITS FOR THE DRAIN. The queue and its breaker live in the TS
/// scheduler's process memory and cross with the scheduler slice of this
/// batch; until then a Rust-filed notification lands in-app, rings the bell,
/// and nudges the brief, and its email delivery is deferred — `will_mail` is
/// still computed, so the moment the drain crosses the leg is already wired
/// correctly. No live workspace exists in the window, and every caller of this
/// function today is itself a port waiting on the scheduler.
pub async fn add_notification(
    deps: &NotifyDeps,
    user_id: &str,
    n: &NotificationInput<'_>,
) -> Result<(), sqlx::Error> {
    let class = notify_class_of(n.kind);
    // Both reads are forgiving (prefs_blob, get_setting's fallback), and both
    // fall back to the answer that sends LESS mail.
    let blob = prefs_blob(&deps.pg, user_id).await.unwrap_or(Value::Null);
    let route = resolve_prefs(&blob)
        .get(class)
        .and_then(|v| v.as_str())
        .unwrap_or("in_app")
        .to_string();
    let wants_mail = matches!(route.as_str(), "email" | "both");
    let will_mail = wants_mail && get_notify_delivery(&deps.pg).await;

    let (notification_id,): (String,) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href, read_at) \
         values ($1::uuid, $2, $3, $4, $5, null) returning id::text",
    )
    .bind(user_id)
    .bind(n.kind)
    .bind(n.title)
    .bind(n.body.unwrap_or(""))
    .bind(n.href.unwrap_or(""))
    .fetch_one(&deps.pg)
    .await?;

    // PUBLISH, NOW THAT THERE IS SOMETHING TO PUBLISH. This is the row's
    // second job after existing: every surface that shows notifications — the
    // bell, the brief's 'worth knowing' section — learns the row landed the
    // moment it does, instead of discovering it on the next poll. Detached
    // both halves: the row is written, which is the part the caller needed,
    // and neither fan-out may cost the request it rode in on.
    publish_user(
        &deps.realtime,
        user_id,
        &UserEvent::Notification {
            notification_id: notification_id.clone(),
        },
    );
    let nudge = deps.clone();
    let nudged = user_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = mark_brief_stale(&nudge, &[nudged]).await {
            tracing::error!("[notifications] brief nudge failed: {e}");
        }
    });
    if will_mail {
        // `both` deliberately passes null: that route wants the in-app copy to
        // stay unread whatever the mail does. The enqueue itself is the drain
        // slice's to write — see the doc above.
    }
    Ok(())
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
