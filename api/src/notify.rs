// Notifications — ui/src/server/notifications.ts, whole: the user's inbox
// (list, unread count, mark-read), the per-user routing table in
// users.notify_prefs, the instance email master switch in app_settings, the
// single writer (`add_notification`) every filing of a row goes through, and
// the mail plane — the one door out, the outbox, the breaker, and the drain
// the scheduler ticks. The transport itself (SMTP/Resend) and the shared mail
// chrome live in email.rs; the digest builds on the same door.
//
// The class vocabulary is lib/notify-classes.ts ported whole: classes,
// fallbacks, the digest's reserved key. resolve/digest DERIVE the effective
// answer here exactly as there, from the same jsonb, so a hand-edited row
// degrades identically on both runtimes.

use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};

use futures_util::future::BoxFuture;

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
/// MAIL IS QUEUED, NEVER SENT HERE. The caller is servicing somebody else's
/// request; it must not wait on, or fail with, an SMTP timeout. Everything on
/// this path is a database write on a connection it was going to use anyway
/// plus a push into the outbox below.
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
    // The gate read is the 10s-cached one (TS getNotifyDelivery without
    // `fresh`): a notification is filed on request paths, and the drain
    // re-reads fresh anyway, so the cache's whole job is saving a settings
    // query per mention.
    let will_mail = wants_mail && delivery_gate(&deps.pg).await;

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
        // stay unread whatever the mail does.
        outbox().enqueue(
            user_id,
            n,
            if route == "email" {
                Some(notification_id)
            } else {
                None
            },
        );
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

// ── THE ONE DOOR OUT ─────────────────────────────────────────────────────────
//
// Every mail Talaria sends ON BEHALF OF A NOTIFICATION OR A DIGEST leaves
// through `send_gated_mail`, and nothing else in this crate may call the
// transport for that purpose. scripts/check-invariants.mjs fails the TS build
// on a fifth caller of sendEmail, because this is the second time this exact
// bug has been written:
//
//   · `addNotification` asked the switch, so notification mail was gated.
//   · `runDigest` called sendEmail directly and never asked, so an admin who
//     turned email OFF still got a daily digest mailed to every user in the
//     workspace — from a control that is named "email delivery", is audited,
//     and did not do what its name said.
//
// The fix is not another `if enabled` at the second call site. Two copies of
// a rule is how the first one drifts. It is one function both paths must go
// through (and the digest's port crosses through this same door).
//
// It reads the switch FRESH on every single mail, not once per batch. A kill
// switch you throw at 03:00 has to stop the drain that is already running, not
// the one after it — and one small settings read per mail is nothing next to
// an SMTP round trip.

fn wall_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The switch read that can FAIL. get_setting (above) swallows a broken read
/// into its fallback — exactly right for the settings panel, exactly wrong
/// here, where "off because the read broke" and "off because an admin said
/// so" must never be confused: the first keeps the queue, the second
/// discards it. Err is the unreadable switch.
async fn read_delivery_switch(pg: &PgPool) -> Result<bool, sqlx::Error> {
    let stored: Option<Value> = sqlx::query_scalar("select value from app_settings where key = $1")
        .bind(DELIVERY_KEY)
        .fetch_optional(pg)
        .await?;
    // Anything that is not exactly `true` is off. A hand-edited settings row
    // must not be able to turn mail on by accident.
    Ok(stored.as_ref().and_then(|v| v.get("emailEnabled")) == Some(&Value::Bool(true)))
}

/// TS getNotifyDelivery() — the 10s-cached read `add_notification` gates on.
/// A failed read is off (the answer that sends less mail) and is NOT cached:
/// a blip must not pin the wrong answer for ten seconds.
const DELIVERY_TTL_MS: i64 = 10_000;
static DELIVERY_GATE: LazyLock<Mutex<Option<(i64, bool)>>> = LazyLock::new(|| Mutex::new(None));

async fn delivery_gate(pg: &PgPool) -> bool {
    let now = wall_ms();
    if let Some((at, v)) = *DELIVERY_GATE.lock().unwrap_or_else(|p| p.into_inner())
        && now - at < DELIVERY_TTL_MS
    {
        return v;
    }
    match read_delivery_switch(pg).await {
        Ok(v) => {
            *DELIVERY_GATE.lock().unwrap_or_else(|p| p.into_inner()) = Some((now, v));
            v
        }
        Err(_) => false,
    }
}

/// What send_gated_mail reports. `blocked` is the instance switch being OFF —
/// explicitly NOT a failure: nothing was attempted, no transport was involved,
/// and nothing should be retried until an admin turns it back on. Callers
/// that count failures (the outbox breaker, the digest's retry) must not
/// count it.
#[derive(Clone, Debug, PartialEq)]
pub struct GatedSendResult {
    pub ok: bool,
    pub blocked: bool,
    pub error: Option<String>,
}

// One log line a minute is enough to say "mail is off and things are being
// refused". Per mail, on a workspace-wide digest pass, it is the outage.
static BLOCKED_WARN: LazyLock<Mutex<(i64, i64)>> = LazyLock::new(|| Mutex::new((0, 0)));

/// One mail offered to the door — the TS sendGatedMail object param.
pub struct GatedMail<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub html: &'a str,
    pub text: Option<&'a str>,
    /// Becomes `List-Unsubscribe` — every mail that goes to a person because
    /// of a standing preference needs it. Callers pass the settings URL or
    /// nothing: a header pointing at a URL that does not resolve is worse
    /// than no header, so the no-verified-domain case omits it.
    pub unsubscribe_url: Option<&'a str>,
    /// What is being refused, for the log line.
    pub what: &'a str,
}

/// Send one piece of notification-or-digest mail, IF this instance is allowed
/// to send mail at all.
pub async fn send_gated_mail(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    mail: &GatedMail<'_>,
) -> GatedSendResult {
    let on = match read_delivery_switch(pg).await {
        Ok(on) => on,
        Err(e) => {
            // We do not know whether we are allowed to send, so we do not
            // send. Failing OPEN here would mean a transient database blip
            // mails the workspace out of an instance whose operator had
            // switched mail off — the one outcome that cannot be taken back.
            tracing::error!(
                "[mail] could not read the delivery switch — not sending \"{}\": {e}",
                mail.what
            );
            return GatedSendResult {
                ok: false,
                blocked: false,
                error: Some("could not read the email delivery switch".into()),
            };
        }
    };
    if !on {
        let mut w = BLOCKED_WARN.lock().unwrap_or_else(|p| p.into_inner());
        w.1 += 1;
        let now = wall_ms();
        if now - w.0 > 60_000 {
            tracing::warn!(
                "[mail] email delivery is OFF for this instance — refused {} mail(s), most recently \"{}\". An admin can turn it back on under Settings → Notifications.",
                w.1,
                mail.what
            );
            *w = (now, 0);
        }
        return GatedSendResult {
            ok: false,
            blocked: true,
            error: Some("email delivery is switched off for this instance".into()),
        };
    }
    let mut input = crate::email::EmailInput::new(mail.to, mail.subject, mail.html, mail.text);
    if let Some(url) = mail.unsubscribe_url {
        input
            .headers
            .push(("List-Unsubscribe".into(), format!("<{url}>")));
    }
    match crate::email::send_email(pg, sb, &input).await {
        crate::email::SendOutcome::Sent => GatedSendResult {
            ok: true,
            blocked: false,
            error: None,
        },
        crate::email::SendOutcome::Failed(e) => GatedSendResult {
            ok: false,
            blocked: false,
            error: Some(e),
        },
    }
}

// ── The outbox ───────────────────────────────────────────────────────────────
//
// Serial, not a fan of parallel sends: a ticket that notifies eight watchers
// would otherwise open eight SMTP connections at once, and a burst is exactly
// when a provider starts rate-limiting.
//
// What this replaced (on the TS side) was a single promise chain with
// `.then(send)` appended per notification — no depth, so a dead SMTP server
// did not stop anything, it accumulated, and the only bound on the whole
// structure was the heap. So: a real queue, with the four things a queue
// needs to be safe.
//   DEPTH    — MAX_QUEUED, and past it new mail is refused at the door.
//   DROP     — refuse the NEWEST rather than evicting the oldest. Load
//              shedding at the entrance is predictable and it never throws
//              away work already accepted. The in-app row has landed either
//              way, so a dropped mail costs a notification its second
//              delivery channel, not its existence.
//   BREAKER  — after enough consecutive failures the drain stops trying for a
//              while. Without it a dead transport burns the full send deadline
//              per item forever, on every tick, and the log is the outage.
//   COUNTERS — sent, failed, dropped, discarded. A queue you cannot see is
//              indistinguishable from a queue that is working.

/// How many mails may wait. Sized for the realistic worst case — an agent
/// fleet fanning a burst of problems out to every admin — and small enough
/// that the memory is irrelevant. If this is ever hit, the answer is not a
/// bigger number (the transport is not keeping up and a larger queue only
/// delays the same outcome) — it is the alert the log line is there to
/// produce.
pub const MAX_QUEUED: usize = 500;

/// How long one drain may spend sending before it yields to the next tick.
/// Shorter than the job's interval so a drain does not routinely overlap its
/// own schedule.
pub const DRAIN_BUDGET_MS: i64 = 25_000;

/// Consecutive failures before the breaker opens, and how long it stays open.
/// Five because a single bad address should not stop everyone else's mail,
/// and five in a row is a transport, not an address.
pub const BREAKER_AFTER: u32 = 5;
pub const BREAKER_COOLDOWN_MS: i64 = 5 * 60_000;

/// How many times one mail is offered to the transport before it is given up
/// on. Three because the failures worth retrying are the transient ones — a
/// timeout, a 429, a provider blip — and a bad address fails identically
/// three times and costs nothing but three log lines. Past it the mail is
/// abandoned OUT LOUD, and the notification is still sitting unread in the
/// recipient's in-app inbox, which is the whole point of not pre-filing it
/// read.
pub const MAX_SEND_ATTEMPTS: u32 = 3;

/// One accepted mail. The notification fields are OWNED copies — the queue
/// outlives the request that filed the row. Clone because the drain PEEKS: the
/// send works on a clone while the row itself stays in the queue for the
/// whole await.
#[derive(Clone)]
struct QueuedMail {
    user_id: String,
    kind: String,
    title: String,
    body: Option<String>,
    href: Option<String>,
    queued_at: i64,
    /// The notification row this mail duplicates, when — and ONLY when — the
    /// row is to be filed read once the mail is actually accepted by a
    /// provider. None for `both`, where the in-app copy is wanted unread
    /// either way. An id and not a boolean because the read-mark happens
    /// AFTER delivery, in the drain, not at insert time.
    mark_read_id: Option<String>,
    /// How many times a transport has already refused this one.
    attempts: u32,
}

struct OutboxState {
    queue: VecDeque<QueuedMail>,
    dropped: usize,
    discarded: usize,
    sent: usize,
    failed: usize,
    /// Retried to the limit and given up on. Distinct from `failed`, which
    /// counts ATTEMPTS: one abandoned mail is MAX_SEND_ATTEMPTS failures.
    abandoned: usize,
    consecutive_failures: u32,
    /// Epoch ms before which the drain does not attempt a send.
    paused_until: i64,
}

/// The queue plus its two locks. The std mutex covers only short synchronous
/// sections (push, peek, shift, counters) and is NEVER held across an await —
/// that is what makes enqueue cheap enough to call from a request path while
/// a drain is mid-send. The tokio mutex below it is the serial consumer.
pub struct Outbox {
    state: Mutex<OutboxState>,
    /// One drain at a time. TS hands a concurrent caller the in-flight
    /// promise (shared result); here the second caller waits for the first
    /// pass to finish and then finds whatever is left. The invariant that
    /// matters — never two drains sending at once — is identical either way.
    serial: tokio::sync::Mutex<()>,
}

fn outbox_state() -> OutboxState {
    OutboxState {
        queue: VecDeque::new(),
        dropped: 0,
        discarded: 0,
        sent: 0,
        failed: 0,
        abandoned: 0,
        consecutive_failures: 0,
        paused_until: 0,
    }
}

impl Outbox {
    /// Accept one mail for later sending, or refuse it. NEVER sends, never
    /// blocks, never fails — this is called from a request path.
    fn enqueue(&self, user_id: &str, n: &NotificationInput<'_>, mark_read_id: Option<String>) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if s.queue.len() >= MAX_QUEUED {
            s.dropped += 1;
            // The first drop is the interesting one; after that, one line per
            // hundred is enough to show it is still happening without
            // becoming the outage.
            if s.dropped == 1 || s.dropped.is_multiple_of(100) {
                tracing::error!(
                    "[notifications] mail queue is full at {MAX_QUEUED} — dropping \"{}\" for user {user_id} ({} dropped since boot). The in-app notification still landed. The transport is not keeping up.",
                    n.kind,
                    s.dropped
                );
            }
            return;
        }
        s.queue.push_back(QueuedMail {
            user_id: user_id.to_string(),
            kind: n.kind.to_string(),
            title: n.title.to_string(),
            body: n.body.map(str::to_string),
            href: n.href.map(str::to_string),
            queued_at: wall_ms(),
            mark_read_id,
            attempts: 0,
        });
    }

    /// Send what is queued, within a time budget. See `drain_notification_mail`.
    async fn drain(&self, deps: &DrainDeps, budget_ms: i64) -> MailDrainResult {
        let _serial = self.serial.lock().await;
        let mut result = {
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            MailDrainResult {
                sent: 0,
                failed: 0,
                requeued: 0,
                abandoned: 0,
                discarded: 0,
                remaining: s.queue.len(),
                dropped: s.dropped,
                paused: false,
            }
        };

        // Read the switch FRESH, every drain, and treat it as authoritative
        // over anything already queued. "Off" has to mean no mail leaves —
        // including the mail that was accepted while it was still on. That is
        // what makes it a kill switch rather than a setting that takes effect
        // eventually.
        let on = match (deps.delivery)().await {
            Ok(on) => on,
            Err(e) => {
                // The switch could not be read, so we do not know whether we
                // are allowed to send. Fail CLOSED and keep the queue: sending
                // on an unread switch is the one outcome that cannot be taken
                // back, and a transient database blip must not also destroy
                // the backlog.
                tracing::error!(
                    "[notifications] could not read the delivery switch — sending nothing this pass: {e}"
                );
                return result;
            }
        };
        if !on {
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if !s.queue.is_empty() {
                result.discarded = s.queue.len();
                s.discarded += result.discarded;
                // Dropped rather than held. Held mail is a trap: an admin who
                // turns email on for the first time three months in would get
                // the entire backlog of every mention since boot delivered in
                // one burst, which is precisely the mass-mailing this switch
                // exists to prevent.
                let n = result.discarded;
                s.queue.clear();
                tracing::warn!(
                    "[notifications] email delivery is off — discarded {n} queued mail(s). Every one of them is still in its recipient's in-app inbox."
                );
            }
            result.remaining = 0;
            return result;
        }

        if (deps.now)() < self.paused_until() {
            result.paused = true;
            return result;
        }

        let deadline = (deps.now)() + budget_ms;
        // Bounded by what was queued when this pass STARTED. A failed mail
        // goes back on the queue now (see below) and without this bound a
        // transport that is refusing everything would spin the same handful of
        // items round the loop for the whole budget instead of yielding to the
        // next tick. Each item gets at most one attempt per drain.
        let mut offers = {
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            s.queue.len()
        };
        while offers > 0 {
            offers -= 1;
            // PEEKED, not popped. What the TS version replaced took the item
            // off the queue before the send, so a failure destroyed it: three
            // mails timed out and simply vanished — nothing requeued, nothing
            // retried, and no in-app trace of them either. Nothing leaves this
            // queue until it has been delivered, refused by the switch, or
            // given up on out loud. (The clone is the peek: the row itself
            // stays in the queue for the whole await.)
            let Some(item) = self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .queue
                .front()
                .cloned()
            else {
                break;
            };
            if (deps.now)() >= deadline {
                break;
            }
            let r = (deps.send)(QueuedMailPayload::from(&item)).await;
            if r.blocked {
                // The switch went off WHILE this drain was running. Same rule
                // as the top of this function, applied to the mail in hand as
                // well as the backlog: stop now, throw the rest away, and
                // never report a refused mail as sent. `item` was PEEKED, not
                // popped, so it is discarded with the rest.
                let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                let lost = s.queue.len();
                result.discarded += lost;
                s.discarded += lost;
                s.queue.clear();
                tracing::warn!(
                    "[notifications] email delivery was switched off mid-drain — discarded {lost} queued mail(s). Every one of them is still in its recipient's in-app inbox."
                );
                break;
            }
            if r.ok {
                {
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.queue.pop_front();
                    s.sent += 1;
                    s.consecutive_failures = 0;
                    result.sent += 1;
                }
                // `delivered` and `ok` are not the same answer: a recipient
                // with no address is `ok` and nothing left the building, so
                // the row must stay unread. Only a message a provider ACCEPTED
                // files it read.
                if r.delivered
                    && let Some(id) = item.mark_read_id
                {
                    (deps.mark_read)(id).await;
                }
            } else {
                let mut item = item;
                item.attempts += 1;
                let trip_breaker;
                {
                    let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
                    s.queue.pop_front();
                    s.failed += 1;
                    s.consecutive_failures += 1;
                    result.failed += 1;
                    if item.attempts < MAX_SEND_ATTEMPTS {
                        // Back of the queue, not the front: everything else
                        // queued deserves an attempt before this one gets a
                        // second. Pushed unconditionally even at MAX_QUEUED —
                        // it was accepted once already, and refusing it here
                        // would be the silent loss this whole design is about.
                        s.queue.push_back(item.clone());
                        result.requeued += 1;
                    } else {
                        s.abandoned += 1;
                        result.abandoned += 1;
                        tracing::error!(
                            "[notifications] giving up on the \"{}\" email for user {} after {} attempts (last error: {}). The notification itself is unread in their in-app inbox — the email was lost, the notification was not.",
                            item.kind,
                            item.user_id,
                            item.attempts,
                            r.error.as_deref().unwrap_or("unknown error")
                        );
                    }
                    trip_breaker = s.consecutive_failures >= BREAKER_AFTER;
                    if trip_breaker {
                        s.paused_until = (deps.now)() + BREAKER_COOLDOWN_MS;
                        result.paused = true;
                        let left = s.queue.len();
                        tracing::error!(
                            "[notifications] {} sends failed in a row — pausing delivery for 5 minutes. {left} mail(s) still queued; they will be attempted after the pause, or dropped if the queue fills first.",
                            s.consecutive_failures
                        );
                    }
                }
                if trip_breaker {
                    break;
                }
            }
        }

        result.remaining = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .queue
            .len();
        result
    }

    fn paused_until(&self) -> i64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .paused_until
    }

    fn stats(&self, now: i64) -> NotificationMailStats {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let oldest = s
            .queue
            .iter()
            .map(|m| m.queued_at)
            .min()
            .map(|at| (now - at).max(0));
        NotificationMailStats {
            queued: s.queue.len(),
            capacity: MAX_QUEUED,
            oldest_queued_ms: oldest,
            sent: s.sent,
            failed: s.failed,
            abandoned: s.abandoned,
            dropped: s.dropped,
            discarded: s.discarded,
            paused_for_ms: (s.paused_until - now).max(0),
        }
    }
}

/// The process outbox — one per process, like the TS globalThis slot, because
/// the job that drains it is `per_instance` by the same reasoning.
pub fn outbox() -> &'static Outbox {
    static OUTBOX: LazyLock<Outbox> = LazyLock::new(|| Outbox {
        state: Mutex::new(outbox_state()),
        serial: tokio::sync::Mutex::new(()),
    });
    &OUTBOX
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MailDrainResult {
    pub sent: usize,
    pub failed: usize,
    /// Failed this pass and put BACK on the queue for the next one.
    pub requeued: usize,
    /// Failed MAX_SEND_ATTEMPTS times and given up on. The notification is
    /// still unread in the recipient's inbox; only the email was lost.
    pub abandoned: usize,
    /// Queued mail thrown away because delivery is switched off.
    pub discarded: usize,
    /// Still queued when this drain gave up its slot.
    pub remaining: usize,
    /// Refused at the door since boot.
    pub dropped: usize,
    /// The breaker was open, so nothing was attempted.
    pub paused: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NotificationMailStats {
    pub queued: usize,
    /// How many the queue may hold before it starts refusing at the door, so
    /// a reader can say "412 of 500" instead of picking its own idea of "a
    /// lot".
    pub capacity: usize,
    /// How long the oldest thing on the queue has been waiting. THE number
    /// for a stuck outbox: depth alone cannot tell a busy minute from a queue
    /// that has not moved since Tuesday, and this can. None when the queue is
    /// empty.
    pub oldest_queued_ms: Option<i64>,
    pub sent: usize,
    pub failed: usize,
    pub abandoned: usize,
    pub dropped: usize,
    pub discarded: usize,
    /// The breaker is open — the drain is short-circuiting and nothing is
    /// being attempted until this reaches zero.
    pub paused_for_ms: i64,
}

/// What the outbox is doing, for verification and for anything that wants to
/// report on it (server/alerts.ts turns these numbers into the observability
/// surface). PROCESS-LOCAL, like the queue it describes, so it reports on the
/// instance that answered the request.
pub fn notification_mail_stats() -> NotificationMailStats {
    outbox().stats(wall_ms())
}

/// What one send-attempt reports back to the drain. `delivered` separates
/// "nothing went wrong" from "a provider took it" — only the second may file
/// the in-app row read, and the address-less recipient is exactly the case
/// where the two answers differ.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendOneResult {
    pub ok: bool,
    pub blocked: bool,
    pub delivered: bool,
    pub error: Option<String>,
}

// The drain's four edges, injectable so the whole queue machinery runs in
// tests with no database, no transport and no clock. Production wiring is
// `real_drain_deps`.
pub type DeliveryFn = Arc<dyn Fn() -> BoxFuture<'static, Result<bool, String>> + Send + Sync>;
pub type SendOneFn =
    Arc<dyn Fn(QueuedMailPayload) -> BoxFuture<'static, SendOneResult> + Send + Sync>;
pub type MarkReadFn = Arc<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The mail as the send edge receives it — the queued row minus its queue
/// bookkeeping (attempts, mark_read_id), owned end to end so it crosses the
/// boxed future without borrowing it.
pub struct QueuedMailPayload {
    pub user_id: String,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub href: Option<String>,
}

impl From<&QueuedMail> for QueuedMailPayload {
    fn from(m: &QueuedMail) -> Self {
        QueuedMailPayload {
            user_id: m.user_id.clone(),
            kind: m.kind.clone(),
            title: m.title.clone(),
            body: m.body.clone(),
            href: m.href.clone(),
        }
    }
}

pub struct DrainDeps {
    pub delivery: DeliveryFn,
    pub send: SendOneFn,
    pub mark_read: MarkReadFn,
    pub now: NowFn,
}

/// Send what is queued, within a time budget — called by the notification-mail
/// job and safe to call directly. Concurrent callers serialize: the queue is a
/// single serial consumer by design, and two drains would be the
/// parallel-connection burst the queue exists to avoid.
pub async fn drain_notification_mail(deps: &DrainDeps, budget_ms: i64) -> MailDrainResult {
    outbox().drain(deps, budget_ms).await
}

// ── The email itself ─────────────────────────────────────────────────────────

/// This deployment's canonical base URL, or None when it has no VERIFIED
/// domain (instance.ts instanceBaseUrl). Every absolute in-app link in a mail
/// is built on it — the digest's deep links and its button, the notification
/// mail's settings link — so "no verified domain" has to mean "no links",
/// never "links that do not resolve".
pub(crate) async fn instance_base_url(pg: &PgPool) -> Option<String> {
    let cfg = crate::instance::get_instance_domain(pg).await;
    let verified = cfg.get("verified") == Some(&Value::Bool(true));
    let domain = cfg.get("domain").and_then(|d| d.as_str()).unwrap_or("");
    if !verified || domain.is_empty() {
        return None;
    }
    Some(format!("https://{domain}"))
}

/// Absolute URL for an in-app path, or None when this deployment has no
/// verified domain to build one from.
async fn app_url(pg: &PgPool, path: &str) -> Option<String> {
    let slash = if path.starts_with('/') { "" } else { "/" };
    instance_base_url(pg)
        .await
        .map(|base| format!("{base}{slash}{path}"))
}

pub const NOTIFY_SETTINGS_PATH: &str = "/settings/notifications";

/// The one sentence that goes in every notification mail's "why am I getting
/// this" slot and its text part's closer.
const WHAT_IS_THIS: &str = "Sent by Talaria because of your notification settings";

struct EmailParts {
    subject: String,
    html: String,
    text: String,
}

/// Compose one notification email from the house style (email_shell,
/// email_button, the escaping) in email.rs. Pure — the deep link and the
/// settings URL are resolved by the caller because they are instance facts,
/// not composition facts.
fn notification_email_parts(
    title: &str,
    body: Option<&str>,
    href: Option<&str>,
    deep_link: Option<&str>,
    settings_url: Option<&str>,
) -> EmailParts {
    use crate::email::{email_button, email_escape, email_shell};
    let body = body.unwrap_or("").trim();
    let cta = match deep_link {
        Some(link) => email_button(link, "Open in Talaria"),
        // No verified instance domain, so no absolute link exists. Say where
        // it is rather than shipping a dead <a href="/boards/…"> that resolves
        // against the mail client.
        None if !href.unwrap_or("").is_empty() => format!(
            "<p style=\"margin:24px 0;font-size:13px;color:#8a8a84\">Open Talaria and go to <code>{}</code>.</p>",
            email_escape(href.unwrap_or(""))
        ),
        None => String::new(),
    };
    let footer = format!(
        "{}{}",
        email_escape(WHAT_IS_THIS),
        match settings_url {
            Some(u) => format!(
                " · <a href=\"{}\" style=\"color:#8a8a84\">change what Talaria emails you</a>",
                email_escape(u)
            ),
            None => " · change what Talaria emails you under Settings → Notifications".to_string(),
        }
    );
    let content_body = if body.is_empty() {
        String::new()
    } else {
        format!(
            "<p style=\"white-space:pre-wrap\">{}</p>",
            email_escape(body)
        )
    };
    let text = [
        Some(title.to_string()),
        (!body.is_empty()).then(|| body.to_string()),
        deep_link
            .map(str::to_string)
            .or_else(|| href.filter(|h| !h.is_empty()).map(str::to_string)),
        Some(format!(
            "{WHAT_IS_THIS}. Change what Talaria emails you: {}",
            settings_url.unwrap_or("Settings → Notifications")
        )),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n\n");
    EmailParts {
        subject: title.to_string(),
        html: email_shell(
            &email_escape(title),
            &format!("{content_body}{cta}"),
            &footer,
        ),
        text,
    }
}

/// Send one notification as mail. Resolves the recipient, builds the deep
/// link, and REPORTS every reason it couldn't — a person who turned email on
/// and hears nothing must be able to find out why from the server log.
/// Never fails: a send is the last thing that should be able to take down the
/// loop draining the queue.
///
/// A recipient with no address is `ok` without `delivered`. It is not a
/// delivery failure and must not count toward the breaker: no number of
/// address-less users means the mail server is broken. `blocked` is the third
/// outcome and it is neither — the instance switch went off, and the drain
/// stops on it rather than counting it, because "sent" must never name a mail
/// that was refused at the gate.
async fn send_notification_email(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    m: &QueuedMailPayload,
) -> SendOneResult {
    // The recipient lookup failing is a FAILED SEND, not "no address": TS
    // wraps the whole body in a catch so anything that breaks composition
    // counts toward the breaker instead of escaping into the drain.
    let to = match sqlx::query_scalar::<_, Option<String>>(
        "select email from users where id = $1::uuid",
    )
    .bind(&m.user_id)
    .fetch_optional(pg)
    .await
    {
        Err(e) => {
            tracing::error!(
                "[notifications] could not build the \"{}\" email for user {}: {e}",
                m.kind,
                m.user_id
            );
            return SendOneResult {
                ok: false,
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
        Ok(row) => row
            .flatten()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty()),
    };
    let Some(to) = to else {
        tracing::warn!(
            "[notifications] no email address for user {} — \"{}\" stays in-app only",
            m.user_id,
            m.kind
        );
        return SendOneResult {
            ok: true,
            ..Default::default()
        };
    };
    let href = m.href.as_deref().filter(|h| !h.is_empty());
    let deep_link = match href {
        Some(h) => app_url(pg, h).await,
        None => None,
    };
    if href.is_some() && deep_link.is_none() {
        tracing::warn!(
            "[notifications] no verified instance domain — sending mail without a clickable link (Admin → Org → Domain)"
        );
    }
    let settings_url = app_url(pg, NOTIFY_SETTINGS_PATH).await;
    let parts = notification_email_parts(
        &m.title,
        m.body.as_deref(),
        href,
        deep_link.as_deref(),
        settings_url.as_deref(),
    );
    let what = format!("notification \"{}\"", m.kind);
    let r = send_gated_mail(
        pg,
        sb,
        &GatedMail {
            to: &to,
            subject: &parts.subject,
            html: &parts.html,
            text: Some(&parts.text),
            unsubscribe_url: settings_url.as_deref(),
            what: &what,
        },
    )
    .await;
    // A refusal by the master switch is not a transport failure and must not
    // count toward the breaker: the mail server is fine, the operator said no.
    if !r.ok && !r.blocked {
        tracing::error!(
            "[notifications] email to {to} for \"{}\" failed: {}",
            m.kind,
            r.error.as_deref().unwrap_or("unknown error")
        );
    }
    SendOneResult {
        ok: r.ok,
        blocked: r.blocked,
        delivered: r.ok,
        error: r.error,
    }
}

/// The mail is out, so the row it duplicates can be filed read. Only ever
/// called after a provider ACCEPTED the message — never for a mail that was
/// queued, dropped, discarded, refused by the master switch, or sent to a
/// user with no address. A notification whose email did not go out has to
/// stay unread, because the inbox is then the only place the person will ever
/// see it.
async fn mark_read_on_delivery(pg: &PgPool, notification_id: &str) {
    if let Err(e) = sqlx::query(
        "update notifications set read_at = now() \
         where id = $1::uuid and read_at is null",
    )
    .bind(notification_id)
    .execute(pg)
    .await
    {
        // Harmless in the direction that matters: the row stays unread, so
        // the person sees it twice rather than not at all.
        tracing::error!(
            "[notifications] mailed {notification_id} but could not file the row read: {e}"
        );
    }
}

/// Production drain edges over the pool + secretbox. The flip's boot path
/// builds these (the secretbox loads through AppState) and registers the job.
pub fn real_drain_deps(pg: PgPool, sb: crate::secretbox::SecretBox) -> Arc<DrainDeps> {
    let delivery: DeliveryFn = {
        let pg = pg.clone();
        Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { read_delivery_switch(&pg).await.map_err(|e| e.to_string()) })
        })
    };
    let send: SendOneFn = {
        let pg = pg.clone();
        let sb = sb.clone();
        Arc::new(move |m: QueuedMailPayload| {
            let pg = pg.clone();
            let sb = sb.clone();
            Box::pin(async move { send_notification_email(&pg, &sb, &m).await })
        })
    };
    let mark_read: MarkReadFn = {
        let pg = pg.clone();
        Arc::new(move |id: String| {
            let pg = pg.clone();
            Box::pin(async move { mark_read_on_delivery(&pg, &id).await })
        })
    };
    let now: NowFn = Arc::new(wall_ms);
    Arc::new(DrainDeps {
        delivery,
        send,
        mark_read,
        now,
    })
}

// ── The job ──────────────────────────────────────────────────────────────────

/// Fast enough that a mention email is not noticeably late, slow enough that
/// an empty queue costs nothing. Longer than DRAIN_BUDGET_MS so a full drain
/// does not routinely collide with the next tick.
pub const NOTIFY_MAIL_EVERY_MS: u64 = 30_000;
/// Let a deploy settle before this instance starts mailing anyone. Nothing
/// else may be derived from this number: it is a settle window, not a
/// coordination point.
pub const NOTIFY_MAIL_FIRST_RUN_DELAY_MS: u64 = 20_000;
/// The outside bound for ONE drain. The scheduler treats a run past its bound
/// as HUNG (observed, never aborted), which is the only way a drain that never
/// returns becomes visible to anybody. The real worst case is one full budget
/// plus the send already in flight when the budget expires; two minutes is
/// that, rounded up.
pub const NOTIFY_MAIL_MAX_RUN_MS: u64 = 2 * 60_000;

/// The job's message, pure so the sentence shaping is testable next to the
/// drain it reports on. "Nothing to do" means NOTHING TO DO — an empty queue
/// and a closed breaker. It used to also mean "the breaker is open and N
/// mails are going nowhere", because that pass sends nothing, fails nothing
/// and discards nothing, so a stuck outbox logged the same quiet line as a
/// healthy idle one. A pass that left mail on the queue has something to say,
/// and the two states it can be in — paused, or simply out of budget — are
/// not the same news.
fn job_message(r: &MailDrainResult, oldest_queued_ms: Option<i64>) -> Option<String> {
    if r.sent == 0 && r.failed == 0 && r.discarded == 0 && r.remaining == 0 {
        return None;
    }
    if r.sent == 0 && r.failed == 0 && r.discarded == 0 {
        let waited = oldest_queued_ms
            .map(|ms| {
                format!(
                    ", oldest waiting {} minute(s)",
                    (ms as f64 / 60_000.0).round()
                )
            })
            .unwrap_or_default();
        return Some(if r.paused {
            format!(
                "NOTHING SENT — delivery is paused after repeated failures; {} mail(s) still queued{waited}. Every one of them is still in their recipient's in-app inbox.",
                r.remaining
            )
        } else {
            format!(
                "nothing sent this pass — {} mail(s) still queued{waited}",
                r.remaining
            )
        });
    }
    let mut m = format!("sent {}, failed {}", r.sent, r.failed);
    if r.requeued > 0 {
        m.push_str(&format!(", requeued {} for the next pass", r.requeued));
    }
    if r.abandoned > 0 {
        m.push_str(&format!(
            ", GAVE UP on {} after {} attempts (still unread in-app)",
            r.abandoned, MAX_SEND_ATTEMPTS
        ));
    }
    if r.discarded > 0 {
        m.push_str(&format!(", discarded {} (delivery off)", r.discarded));
    }
    if r.remaining > 0 {
        m.push_str(&format!(", {} still queued", r.remaining));
    }
    if r.dropped > 0 {
        m.push_str(&format!(", {} dropped since boot", r.dropped));
    }
    if r.paused {
        m.push_str(" — delivery is paused after repeated failures");
    }
    Some(m)
}

/// The scheduler owns the timing — this module owns the work. `per_instance`
/// because the queue is in THIS process's memory: a Redis lease would let one
/// instance win the tick and leave every other instance's queue undrained
/// forever.
pub fn notification_mail_job_spec(deps: Arc<DrainDeps>) -> crate::scheduler::JobSpec {
    use crate::scheduler::{JobName, JobSpec};
    JobSpec {
        name: JobName::NotificationMail,
        every_ms: NOTIFY_MAIL_EVERY_MS,
        first_run_delay_ms: Some(NOTIFY_MAIL_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(NOTIFY_MAIL_MAX_RUN_MS),
        per_instance: true,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let r = drain_notification_mail(&deps, DRAIN_BUDGET_MS).await;
                Ok(job_message(&r, notification_mail_stats().oldest_queued_ms))
            })
        }),
    }
}

pub fn register_notification_mail_job(deps: Arc<DrainDeps>) {
    crate::scheduler::register_job(notification_mail_job_spec(deps));
}

/// The members of a channel — who a message in it might be about
/// (daily-brief-stale.ts channelMemberIds).
///
/// Kept here rather than in channels.rs for the cycle reason the TS file
/// states: channels' insert needs the brief nudge, and the brief's delegation
/// layer needs channels' insert. It is one query and it is the only thing this
/// module needs to know about a channel.
pub async fn channel_member_ids(
    pg: &sqlx::PgPool,
    channel_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("select user_id::text from channel_members where channel_id = $1::uuid")
            .bind(channel_id)
            .fetch_all(pg)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// A message landed in a conversation (daily-brief-stale.ts
/// briefsFollowMessage). Detached on purpose: the person who sent it is waiting
/// on the POST that wrote it, and their reply must not block on somebody
/// else's brief bookkeeping.
///
/// WHAT IT DOES NOT DO IS SWEEP. A sweep is four scoped queries and up to one
/// model call, and posting a message in a busy DM would fire one per message
/// per participant. This clears the sweep throttle and rings the bell instead:
/// an open page refetches, the read path sweeps on the way through because the
/// throttle is now clear, and the person sees the line close. A page nobody is
/// looking at costs one UPDATE and picks the change up on the next scheduled
/// pass.
pub fn briefs_follow_message(deps: NotifyDeps, channel_id: String) {
    tokio::spawn(async move {
        match channel_member_ids(&deps.pg, &channel_id).await {
            Ok(ids) => {
                if let Err(e) = mark_brief_stale(&deps, &ids).await {
                    tracing::error!(
                        "[daily-brief] could not follow a message in {channel_id}: {e}"
                    );
                }
            }
            Err(e) => {
                tracing::error!("[daily-brief] could not follow a message in {channel_id}: {e}");
            }
        }
    });
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

    // ── the mail plane ───────────────────────────────────────────────────────

    use std::sync::Mutex as StdMutex;

    /// Everything a drain test wants to steer or observe, behind Arc<Mutex>
    /// so the boxed futures in DrainDeps can reach it.
    struct FakeEdge {
        /// The drain's top-of-pass switch read. Err = unreadable.
        delivery: StdMutex<Result<bool, String>>,
        /// One per send attempt, in order; an empty list answers ok+delivered.
        outcomes: StdMutex<Vec<SendOneResult>>,
        marked_read: StdMutex<Vec<String>>,
        sent_kinds: StdMutex<Vec<String>>,
        clock: StdMutex<i64>,
    }

    impl Default for FakeEdge {
        fn default() -> Self {
            FakeEdge {
                delivery: StdMutex::new(Ok(true)),
                outcomes: StdMutex::new(Vec::new()),
                marked_read: StdMutex::new(Vec::new()),
                sent_kinds: StdMutex::new(Vec::new()),
                clock: StdMutex::new(0),
            }
        }
    }

    impl FakeEdge {
        fn deps(self: &Arc<Self>) -> DrainDeps {
            let delivery: DeliveryFn = {
                let f = Arc::clone(self);
                Arc::new(move || {
                    let f = Arc::clone(&f);
                    Box::pin(async move { f.delivery.lock().unwrap().clone() })
                })
            };
            let send: SendOneFn = {
                let f = Arc::clone(self);
                Arc::new(move |m: QueuedMailPayload| {
                    let f = Arc::clone(&f);
                    Box::pin(async move {
                        f.sent_kinds.lock().unwrap().push(m.kind.clone());
                        let next = {
                            let mut outcomes = f.outcomes.lock().unwrap();
                            (!outcomes.is_empty()).then(|| outcomes.remove(0))
                        };
                        next.unwrap_or(SendOneResult {
                            ok: true,
                            delivered: true,
                            ..Default::default()
                        })
                    })
                })
            };
            // outcomes pops from the FRONT like the queue does.
            let mark_read: MarkReadFn = {
                let f = Arc::clone(self);
                Arc::new(move |id: String| {
                    let f = Arc::clone(&f);
                    Box::pin(async move {
                        f.marked_read.lock().unwrap().push(id);
                    })
                })
            };
            let now: NowFn = {
                let f = Arc::clone(self);
                Arc::new(move || *f.clock.lock().unwrap())
            };
            DrainDeps {
                delivery,
                send,
                mark_read,
                now,
            }
        }
    }

    fn fresh_outbox(n: usize) -> Outbox {
        let o = Outbox {
            state: Mutex::new(outbox_state()),
            serial: tokio::sync::Mutex::new(()),
        };
        for i in 0..n {
            o.enqueue(
                &format!("user-{i}"),
                &NotificationInput {
                    kind: "mention",
                    title: &format!("title {i}"),
                    body: Some("body"),
                    href: Some("/boards/x"),
                },
                (i % 2 == 0).then(|| format!("notif-{i}")),
            );
        }
        o
    }

    fn ok_delivered() -> SendOneResult {
        SendOneResult {
            ok: true,
            delivered: true,
            ..Default::default()
        }
    }

    fn fail(msg: &str) -> SendOneResult {
        SendOneResult {
            ok: false,
            error: Some(msg.into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_off_switch_discards_the_whole_queue() {
        let f = Arc::new(FakeEdge {
            delivery: StdMutex::new(Ok(false)),
            ..Default::default()
        });
        let o = fresh_outbox(3);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 0);
        assert_eq!(r.discarded, 3);
        assert_eq!(r.remaining, 0);
        assert_eq!(o.stats(0).queued, 0);
        // Held mail is a trap; the whole point is that it is GONE.
        assert_eq!(o.stats(0).discarded, 3);
        assert!(f.sent_kinds.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_switch_fails_closed_and_keeps_the_queue() {
        let f = Arc::new(FakeEdge {
            delivery: StdMutex::new(Err("db down".into())),
            ..Default::default()
        });
        let o = fresh_outbox(3);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 0);
        assert_eq!(r.discarded, 0);
        // Nothing sent AND nothing thrown away — the backlog survives the blip.
        assert_eq!(r.remaining, 3);
        assert_eq!(o.stats(0).queued, 3);
    }

    #[tokio::test]
    async fn a_happy_drain_marks_read_only_where_the_route_says_so() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(3);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 3);
        assert_eq!(r.remaining, 0);
        // even indexes queued with a mark_read_id (route 'email'), odd without
        // (route 'both') — only the first kind may be filed read.
        assert_eq!(
            *f.marked_read.lock().unwrap(),
            vec!["notif-0".to_string(), "notif-2".to_string()]
        );
    }

    #[tokio::test]
    async fn an_ok_without_delivery_is_sent_but_never_marks_read() {
        // The address-less recipient: ok, nothing left the building.
        let f = Arc::new(FakeEdge {
            outcomes: StdMutex::new(vec![SendOneResult {
                ok: true,
                delivered: false,
                ..Default::default()
            }]),
            ..Default::default()
        });
        let o = fresh_outbox(1);
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 1);
        assert_eq!(r.failed, 0);
        assert!(f.marked_read.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failing_mail_retries_then_is_abandoned_out_loud() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(1);
        for pass in 1..=MAX_SEND_ATTEMPTS {
            *f.outcomes.lock().unwrap() = vec![fail("smtp timed out")];
            let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
            assert_eq!(r.failed, 1, "pass {pass}");
            if pass < MAX_SEND_ATTEMPTS {
                assert_eq!(r.requeued, 1, "pass {pass}");
                assert_eq!(r.remaining, 1, "pass {pass}");
                assert_eq!(r.abandoned, 0);
            } else {
                assert_eq!(r.abandoned, 1);
                assert_eq!(r.remaining, 0);
            }
        }
        // Three failures for ONE mail never trip the breaker (5 consecutive).
        assert_eq!(o.stats(0).paused_for_ms, 0);
    }

    #[tokio::test]
    async fn five_consecutive_failures_trip_the_breaker_mid_drain() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(7);
        *f.outcomes.lock().unwrap() = vec![fail("x"); 7];
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        // The 5th consecutive failure pauses delivery and stops the pass.
        assert_eq!(r.failed, BREAKER_AFTER as usize);
        assert_eq!(r.requeued, 5);
        assert!(r.paused);
        assert_eq!(r.remaining, 7); // 5 requeued at the back + 2 never offered
        assert!(o.stats(0).paused_for_ms > 0);
        // And the pause holds: a later pass attempts nothing even though the
        // switch is on.
        f.outcomes.lock().unwrap().clear();
        *f.delivery.lock().unwrap() = Ok(true);
        let r2 = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r2.sent, 0);
        assert!(r2.paused);
        // Past the cooldown the same queue is attempted again.
        *f.clock.lock().unwrap() += BREAKER_COOLDOWN_MS + 1;
        let r3 = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r3.sent, 7);
    }

    #[tokio::test]
    async fn a_blocked_send_mid_drain_discards_the_rest() {
        let f = Arc::new(FakeEdge::default());
        let o = fresh_outbox(3);
        *f.outcomes.lock().unwrap() = vec![
            ok_delivered(),
            SendOneResult {
                ok: false,
                blocked: true,
                error: Some("email delivery is switched off for this instance".into()),
                delivered: false,
            },
        ];
        let r = o.drain(&f.deps(), DRAIN_BUDGET_MS).await;
        assert_eq!(r.sent, 1);
        assert_eq!(r.failed, 0);
        // The peeked-but-refused item counts in the discard, and so does the
        // one behind it.
        assert_eq!(r.discarded, 2);
        assert_eq!(r.remaining, 0);
    }

    #[tokio::test]
    async fn the_budget_expires_before_the_queue_does() {
        let f = Arc::new(FakeEdge::default());
        // Each send eats the entire budget, so exactly one goes out per pass.
        let send: SendOneFn = {
            let f = Arc::clone(&f);
            Arc::new(move |m: QueuedMailPayload| {
                let f = Arc::clone(&f);
                Box::pin(async move {
                    f.sent_kinds.lock().unwrap().push(m.kind.clone());
                    *f.clock.lock().unwrap() += 10_000;
                    ok_delivered()
                })
            })
        };
        let o = fresh_outbox(2);
        let deps = DrainDeps {
            delivery: f.deps().delivery,
            send,
            mark_read: f.deps().mark_read,
            now: f.deps().now,
        };
        let r = o.drain(&deps, 10_000).await;
        assert_eq!(r.sent, 1);
        assert_eq!(r.remaining, 1);
    }

    #[test]
    fn a_full_queue_refuses_at_the_door_without_losing_accepted_work() {
        let o = Outbox {
            state: Mutex::new(outbox_state()),
            serial: tokio::sync::Mutex::new(()),
        };
        for i in 0..=MAX_QUEUED {
            o.enqueue(
                &format!("user-{i}"),
                &NotificationInput {
                    kind: "dm",
                    title: "t",
                    body: None,
                    href: None,
                },
                None,
            );
        }
        let s = o.stats(0);
        assert_eq!(s.queued, MAX_QUEUED);
        assert_eq!(s.dropped, 1); // the 501st — the newest — is refused
        assert_eq!(s.capacity, MAX_QUEUED);
        assert!(s.oldest_queued_ms.is_some()); // a stuck-outbox reader needs this
    }

    // ── the sentences ────────────────────────────────────────────────────────

    #[test]
    fn job_message_shapes_the_ts_sentences() {
        // Quiet pass: nothing to do at all.
        assert_eq!(
            job_message(
                &MailDrainResult {
                    ..Default::default()
                },
                None
            ),
            None
        );
        // Paused with mail on the queue: the loud one.
        let paused = MailDrainResult {
            remaining: 3,
            paused: true,
            ..Default::default()
        };
        assert_eq!(
            job_message(&paused, Some(90_000)).as_deref(),
            Some(
                "NOTHING SENT — delivery is paused after repeated failures; 3 mail(s) still queued, oldest waiting 2 minute(s). Every one of them is still in their recipient's in-app inbox."
            )
        );
        // Out of budget: the quiet one, no oldest to name.
        let budget = MailDrainResult {
            remaining: 2,
            ..Default::default()
        };
        assert_eq!(
            job_message(&budget, None).as_deref(),
            Some("nothing sent this pass — 2 mail(s) still queued")
        );
        // The full tally, every clause present and in TS order.
        let full = MailDrainResult {
            sent: 3,
            failed: 1,
            requeued: 1,
            abandoned: 1,
            discarded: 0,
            remaining: 1,
            dropped: 2,
            paused: false,
        };
        assert_eq!(
            job_message(&full, None).as_deref(),
            Some(
                "sent 3, failed 1, requeued 1 for the next pass, GAVE UP on 1 after 3 attempts (still unread in-app), 1 still queued, 2 dropped since boot"
            )
        );
        // …and the paused tail on an otherwise-productive pass.
        let tail = MailDrainResult {
            sent: 2,
            failed: 5,
            paused: true,
            ..Default::default()
        };
        assert_eq!(
            job_message(&tail, None).as_deref(),
            Some("sent 2, failed 5 — delivery is paused after repeated failures")
        );
    }

    #[test]
    fn the_composed_mail_matches_the_ts_arrangement() {
        // Full case: verified domain → deep link, settings URL, real body.
        let p = notification_email_parts(
            "Q3 plan",
            Some("  line one\nline two  "),
            Some("/plans/q3"),
            Some("https://talaria.example/plans/q3"),
            Some("https://talaria.example/settings/notifications"),
        );
        assert_eq!(p.subject, "Q3 plan");
        assert!(p.html.contains(&format!(
            "<p style=\"white-space:pre-wrap\">line one\nline two</p>{}",
            crate::email::email_button("https://talaria.example/plans/q3", "Open in Talaria")
        )));
        assert!(p.html.contains(
            "Sent by Talaria because of your notification settings · <a href=\"https://talaria.example/settings/notifications\" style=\"color:#8a8a84\">change what Talaria emails you</a>"
        ));
        assert_eq!(
            p.text,
            "Q3 plan\n\nline one\nline two\n\nhttps://talaria.example/plans/q3\n\nSent by Talaria because of your notification settings. Change what Talaria emails you: https://talaria.example/settings/notifications"
        );

        // No verified domain: no dead <a href="/…">, say where it is instead,
        // and the footer falls back to the named path.
        let p = notification_email_parts("Ping", Some("hello"), Some("/inbox"), None, None);
        assert!(p.html.contains(
            "<p style=\"margin:24px 0;font-size:13px;color:#8a8a84\">Open Talaria and go to <code>/inbox</code>.</p>"
        ));
        assert!(
            p.html
                .contains(" · change what Talaria emails you under Settings → Notifications")
        );
        assert_eq!(
            p.text,
            "Ping\n\nhello\n\n/inbox\n\nSent by Talaria because of your notification settings. Change what Talaria emails you: Settings → Notifications"
        );

        // No href, no body: the text part is title + closer, nothing else.
        let p = notification_email_parts("Bare", None, None, None, None);
        assert_eq!(
            p.text,
            "Bare\n\nSent by Talaria because of your notification settings. Change what Talaria emails you: Settings → Notifications"
        );
    }
}
