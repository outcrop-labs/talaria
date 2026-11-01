// THE single writer: every filing of a row goes through here.

use serde_json::Value;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;
use crate::brief::mark_brief_stale;
use crate::classes::notify_class_of;
use crate::href::stored_href;
use crate::mail::delivery_gate;
use crate::prefs::{prefs_blob, resolve_prefs};

// ── The write: one notification, filed ───────────────────────────────────────

/// What a writer files: the kind routes through the class table; title, body
/// and href are CONTENT and reach only the person whose row this is.
pub struct NotificationInput<'a> {
    pub kind: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub href: Option<&'a str>,
}

/// THE single writer. Ten call sites file
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
    // The gate read is the 10s-cached one: a notification is filed on request
    // paths, and the drain re-reads fresh anyway, so the cache's whole job is
    // saving a settings query per mention.
    let will_mail = wants_mail && delivery_gate(&deps.pg).await;

    let href = stored_href(n.kind, n.href);
    let (notification_id, created_ms): (String, i64) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href, read_at) \
         values ($1::uuid, $2, $3, $4, $5, null) \
         returning id::text, (trunc(extract(epoch from created_at) * 1000))::bigint",
    )
    .bind(user_id)
    .bind(n.kind)
    .bind(n.title)
    .bind(n.body.unwrap_or(""))
    .bind(&href)
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
    crate::deliver::after_insert(
        user_id,
        n,
        &notification_id,
        created_ms,
        &href,
        &route,
        will_mail,
    );
    Ok(())
}

/// A newer message in a thread that ALREADY has an unread row: that row is
/// brought up to date IN PLACE — same id, the new title and body, and
/// `created_at = now()` — and then announced exactly as a new row would be:
/// published, the brief nudged, pushed with `renotify` so the OS re-shows it
/// under the same tag. Every consumer reads "a known id whose createdAt
/// moved later" as a re-alert (the tab's arrival record, the shell's
/// watcher).
///
/// SAME ID ON PURPOSE. Inbox focus keys its snoozes by the row id, so a
/// delete-and-reinsert would lose them; and one row per conversation is what
/// keeps the bell a list of places, not a transcript.
///
/// NO MAIL. The first row's mail stands; a burst of DMs is one email, not
/// one per line. Answers false when the row is gone or was read meanwhile —
/// the caller files a fresh row then, the ordinary way.
pub async fn refresh_notification(
    deps: &NotifyDeps,
    notification_id: &str,
    user_id: &str,
    n: &NotificationInput<'_>,
) -> Result<bool, sqlx::Error> {
    let href = stored_href(n.kind, n.href);
    let updated: Option<(String, i64)> = sqlx::query_as(
        "update notifications set title = $3, body = $4, href = $5, created_at = now() \
         where id = $1::uuid and user_id = $2::uuid and read_at is null \
         returning id::text, (trunc(extract(epoch from created_at) * 1000))::bigint",
    )
    .bind(notification_id)
    .bind(user_id)
    .bind(n.title)
    .bind(n.body.unwrap_or(""))
    .bind(&href)
    .fetch_optional(&deps.pg)
    .await?;
    let Some((id, created_ms)) = updated else {
        return Ok(false);
    };
    publish_user(
        &deps.realtime,
        user_id,
        &UserEvent::Notification {
            notification_id: id.clone(),
        },
    );
    let nudge = deps.clone();
    let nudged = user_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = mark_brief_stale(&nudge, &[nudged]).await {
            tracing::error!("[notifications] brief nudge failed: {e}");
        }
    });
    crate::deliver::after_refresh(deps, user_id, n, id, created_ms, href).await;
    Ok(true)
}
