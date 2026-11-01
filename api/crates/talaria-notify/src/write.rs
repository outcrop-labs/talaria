// THE single writer: every filing of a row goes through here, in one
// transaction per recipient that takes the person's next notify seq (I-10)
// and inserts — or, with a group key, coalesces into the one open row of that
// group (I-24).

use serde_json::Value;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;
use crate::brief::mark_brief_stale;
use crate::classes::notify_class_of;
use crate::deliver::{self, Decision};
use crate::href::stored_href;
use crate::meta::{Coalesce, Filed, NotifyMeta};

// ── The write: one notification, filed ───────────────────────────────────────

/// What a writer files: the kind routes through the class table; title, body
/// and href are CONTENT and reach only the person whose row this is.
pub struct NotificationInput<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub href: Option<&'a str>,
}

/// `add_notification` behind a per-recipient DAILY cap on one `kind`: the
/// row is written only while the recipient holds fewer than `daily_cap` rows
/// of that kind from the last 24 hours, and the answer says whether it was.
///
/// WHY A CAP BY KIND AND RECIPIENT rather than a source column: the table
/// records who is told, not who caused it, and the kinds an agent can
/// originate (`mention` from an agent post, `agent-problem`, `gap_reported`)
/// are already their own kinds. A cap on the kind is the cap on the flood —
/// 1000 agent posts saying `@jon ping` become fifty rows in Jon's inbox and
/// fifty pushes, not a thousand of each. Read-then-insert, deliberately: two
/// concurrent writes may land one row past the cap, which is the right trade
/// for a brake whose job is "thousands become dozens".
pub async fn add_notification_capped(
    deps: &NotifyDeps,
    user_id: &str,
    n: &NotificationInput<'_>,
    daily_cap: i64,
) -> Result<bool, sqlx::Error> {
    let (count,): (i64,) = sqlx::query_as(
        "select count(*) from notifications \
         where user_id = $1::uuid and kind = $2 and created_at > now() - interval '1 day'",
    )
    .bind(user_id)
    .bind(n.kind)
    .fetch_one(&deps.pg)
    .await?;
    if count >= daily_cap {
        tracing::warn!(
            "[notifications] daily cap of {daily_cap} on kind {:?} reached for {user_id}; dropped",
            n.kind
        );
        return Ok(false);
    }
    add_notification(deps, user_id, n).await?;
    Ok(true)
}

/// THE single writer, as wave 1's twenty-odd call sites file: no group key,
/// no entity, no actor — `add_notification_with` and `NotifyMeta::default()`.
/// The caller's contract is fire and forget: the row is the record, and
/// everything after it is a delivery that must not cost the request it rode
/// in on.
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
/// plus a push into the outbox.
pub async fn add_notification(
    deps: &NotifyDeps,
    user_id: &str,
    n: &NotificationInput<'_>,
) -> Result<(), sqlx::Error> {
    add_notification_with(deps, user_id, n, &NotifyMeta::default())
        .await
        .map(|_| ())
}

/// The insert, or the coalesce into the open row of the group. The DO UPDATE
/// only touches a row that is STILL open: a row read between the conflict
/// check and the update answers no row, and the write is retried as a fresh
/// insert (the read row has left the index).
const UPSERT: &str = "insert into notifications \
       (user_id, kind, title, body, href, seq, group_key, count, updated_at, alert, \
        actor_kind, actor_id, actor_label, entity_type, entity_id, pushed_at) \
     values ($1::uuid, $2, $3, $4, $5, $6, $7, 1, now(), $8, $9, $10, $11, $12, $13, \
             case when $14::boolean then now() end) \
     on conflict (user_id, group_key) \
       where group_key is not null and read_at is null and archived_at is null \
     do update set title = excluded.title, body = excluded.body, href = excluded.href, \
       seq = excluded.seq, count = notifications.count + 1, updated_at = now(), \
       alert = excluded.alert, actor_kind = excluded.actor_kind, \
       actor_id = excluded.actor_id, actor_label = excluded.actor_label, \
       entity_type = coalesce(excluded.entity_type, notifications.entity_type), \
       entity_id = coalesce(excluded.entity_id, notifications.entity_id), \
       created_at = case when $15::boolean then now() else notifications.created_at end, \
       pushed_at = case when $14::boolean and $15::boolean then now() \
                        else notifications.pushed_at end \
     where notifications.read_at is null and notifications.archived_at is null \
     returning id::text, count, (xmax = 0), \
       (trunc(extract(epoch from created_at) * 1000))::bigint";

/// File one row for one person, the way `meta` says (I-24).
///
/// ONE TRANSACTION:
/// (a) `notify_seq + 1` on the person's users row. That row lock is held to
///     commit, so two writers for one person serialize and seq order IS commit
///     order (C2): a reader that sees `notify_seq = N` sees every row with
///     `seq <= N`. No users row → the person is gone → `RowNotFound`.
/// (b) with a key and a window: an open row of the group untouched for longer
///     than the window keeps its content and loses its key.
/// (c) with a key and `adopt_legacy`, when no open row holds the key: the
///     newest open, unkeyed row of the same kind and href (a pre-contract row)
///     takes the key — same id, so its inbox-focus snooze stays with it.
/// (d) insert, or coalesce into the open row.
///
/// Reading or archiving never takes a seq; only alerting writes do. After
/// commit: the `notification` event with the seq, the brief nudge, and
/// `deliver::after_write` (push and mail, wave 1's tail).
pub async fn add_notification_with(
    deps: &NotifyDeps,
    user_id: &str,
    n: &NotificationInput<'_>,
    meta: &NotifyMeta,
) -> Result<Filed, sqlx::Error> {
    let class = notify_class_of(n.kind);
    let href = stored_href(n.kind, n.href);
    let key = meta.group_key.as_deref().filter(|k| !k.is_empty());
    let mut tx = deps.pg.begin().await?;
    // (a) — and the prefs ride the same row read.
    let bumped: Option<(i64, Option<Value>)> = sqlx::query_as(
        "update users set notify_seq = notify_seq + 1 where id = $1::uuid \
         returning notify_seq, notify_prefs",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((seq, blob)) = bumped else {
        tx.rollback().await?;
        return Err(sqlx::Error::RowNotFound);
    };
    let blob = blob.unwrap_or(Value::Null);
    let decision = deliver::decide(&blob, class, meta);
    let window = meta
        .group_window_secs
        .map(|w| w.min(i32::MAX as u32) as i32);
    if let Some(key) = key {
        // (b)
        if let Some(window) = window {
            sqlx::query(
                "update notifications set group_key = null \
                 where user_id = $1::uuid and group_key = $2 \
                   and read_at is null and archived_at is null \
                   and coalesce(updated_at, created_at) < now() - $3::int * interval '1 second'",
            )
            .bind(user_id)
            .bind(key)
            .bind(window)
            .execute(&mut *tx)
            .await?;
        }
        // (c) — never a row the window would expire at once.
        if meta.adopt_legacy {
            sqlx::query(
                "update notifications set group_key = $2 \
                 where id = (select id from notifications \
                             where user_id = $1::uuid and kind = $3 and href = $4 \
                               and group_key is null and read_at is null and archived_at is null \
                               and ($5::int is null \
                                    or coalesce(updated_at, created_at) \
                                       >= now() - $5::int * interval '1 second') \
                             order by created_at desc limit 1) \
                   and not exists (select 1 from notifications \
                                   where user_id = $1::uuid and group_key = $2 \
                                     and read_at is null and archived_at is null)",
            )
            .bind(user_id)
            .bind(key)
            .bind(n.kind)
            .bind(&href)
            .bind(window)
            .execute(&mut *tx)
            .await?;
        }
    }
    // (d)
    let actor = meta.actor.as_ref();
    let entity = meta.entity.as_ref();
    let realert = meta.coalesce == Coalesce::Realert;
    let mut landed: Option<(String, i32, bool, i64)> = None;
    for _ in 0..3 {
        landed = sqlx::query_as(UPSERT)
            .bind(user_id)
            .bind(n.kind)
            .bind(n.title)
            .bind(n.body.unwrap_or(""))
            .bind(&href)
            .bind(seq)
            .bind(key)
            .bind(decision.alert.as_str())
            .bind(actor.map(|a| a.kind.as_str()))
            .bind(actor.map(|a| a.id.as_str()))
            .bind(actor.map(|a| a.label.as_str()))
            .bind(entity.map(|e| e.kind))
            .bind(entity.map(|e| e.id.as_str()))
            .bind(decision.records_push())
            .bind(realert)
            .fetch_optional(&mut *tx)
            .await?;
        if landed.is_some() {
            break;
        }
    }
    let Some((id, count, inserted, created_ms)) = landed else {
        tx.rollback().await?;
        return Err(sqlx::Error::RowNotFound);
    };
    tx.commit().await?;
    let filed = Filed {
        id,
        seq,
        count,
        inserted,
        created_ms,
        alert: decision.alert,
    };
    announce(deps, user_id, &filed.id, seq);
    deliver::after_write(deps, user_id, n, meta, &filed, &decision, &href).await;
    Ok(filed)
}

/// PUBLISH, NOW THAT THERE IS SOMETHING TO PUBLISH. This is the row's second
/// job after existing: every surface that shows notifications — the bell, the
/// brief's 'worth knowing' section — learns the row landed the moment it
/// does, instead of discovering it on the next poll. Detached both halves: the
/// row is written, which is the part the caller needed, and neither fan-out
/// may cost the request it rode in on.
fn announce(deps: &NotifyDeps, user_id: &str, notification_id: &str, seq: i64) {
    publish_user(
        &deps.realtime,
        user_id,
        &UserEvent::Notification {
            notification_id: notification_id.to_string(),
            seq: Some(seq),
        },
    );
    let nudge = deps.clone();
    let nudged = user_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = mark_brief_stale(&nudge, &[nudged]).await {
            tracing::error!("[notifications] brief nudge failed: {e}");
        }
    });
}

/// A newer message in a thread that ALREADY has an unread row: that row is
/// brought up to date IN PLACE — same id, the new title and body, and
/// `created_at = now()` — and then announced exactly as a new row would be:
/// published, the brief nudged, pushed with `renotify` so the OS re-shows it
/// under the same tag. Every consumer reads "a known id whose createdAt
/// moved later" as a re-alert (the tab's arrival record, the shell's
/// watcher). It takes the person's next seq, like every alerting write.
///
/// SAME ID ON PURPOSE. Inbox focus keys its snoozes by the row id, so a
/// delete-and-reinsert would lose them; and one row per conversation is what
/// keeps the bell a list of places, not a transcript.
///
/// NO MAIL. The first row's mail stands; a burst of DMs is one email, not
/// one per line. Answers false when the row is gone or was read meanwhile —
/// the caller files a fresh row then, the ordinary way — and then no seq is
/// spent (the transaction rolls back).
pub async fn refresh_notification(
    deps: &NotifyDeps,
    notification_id: &str,
    user_id: &str,
    n: &NotificationInput<'_>,
) -> Result<bool, sqlx::Error> {
    let class = notify_class_of(n.kind);
    let href = stored_href(n.kind, n.href);
    let mut tx = deps.pg.begin().await?;
    let bumped: Option<(i64, Option<Value>)> = sqlx::query_as(
        "update users set notify_seq = notify_seq + 1 where id = $1::uuid \
         returning notify_seq, notify_prefs",
    )
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((seq, blob)) = bumped else {
        tx.rollback().await?;
        return Ok(false);
    };
    let blob = blob.unwrap_or(Value::Null);
    let decision: Decision = deliver::decide(&blob, class, &NotifyMeta::default());
    let updated: Option<(String, i64)> = sqlx::query_as(
        "update notifications set title = $3, body = $4, href = $5, created_at = now(), \
                seq = $6, updated_at = now(), count = count + 1, \
                pushed_at = case when $7::boolean then now() else pushed_at end \
         where id = $1::uuid and user_id = $2::uuid and read_at is null \
         returning id::text, (trunc(extract(epoch from created_at) * 1000))::bigint",
    )
    .bind(notification_id)
    .bind(user_id)
    .bind(n.title)
    .bind(n.body.unwrap_or(""))
    .bind(&href)
    .bind(seq)
    .bind(decision.records_push())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, created_ms)) = updated else {
        tx.rollback().await?;
        return Ok(false);
    };
    tx.commit().await?;
    announce(deps, user_id, &id, seq);
    deliver::after_refresh(user_id, n, id, created_ms, href, &decision);
    Ok(true)
}
