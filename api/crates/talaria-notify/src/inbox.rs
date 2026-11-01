// The person's inbox: the list (wave 1's rows and the v2 rows, I-11), the
// unread counts, mark-read by ids / href / all, the read-sync event that
// tells their other tabs and devices (I-16), and the entity clears producers
// call when the thing a row is about resolves.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;
use crate::classes::{category_of, for_you_kinds, kind_reason, notify_class_of};
use crate::meta::NotifyEntity;

/// The inbox's own feature flags (I-23). W2-H adds its own here.
pub const FEATURES: &[&str] = &[];

/// The most read ids one read-sync event names; past it the event says
/// `resync` and every client refetches instead.
pub const READ_IDS_MAX: usize = 100;

/// The notification row as the LIST serves it — the select's key order
/// (id, kind, title, body, href, readAt, createdAt). Timestamps ride as
/// epoch-ms → ISO, millisecond precision, UTC.
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

/// The newest 50 rows in the v2 wire shape, AND the person's notify seq, from
/// ONE statement — one snapshot, so every row with a seq at or below the
/// answer had committed when the rows were read (I-10). It is not a complete
/// read through that seq (a Quiet coalesce keeps an older createdAt, and the
/// list stops at 50): only a `?since` read with `more: false` lets a client
/// record `seq` as read through (I-20). A person with no row answers seq 0
/// and no rows.
pub async fn list_inbox(pg: &PgPool, user_id: &str) -> Result<(Vec<Value>, i64), sqlx::Error> {
    let rows = sqlx::query(
        "select u.notify_seq, n.id::text, n.kind, n.title, n.body, n.href, \
                (trunc(extract(epoch from n.read_at) * 1000))::bigint, \
                (trunc(extract(epoch from n.created_at) * 1000))::bigint, \
                n.seq, n.alert, n.count, \
                (trunc(extract(epoch from n.updated_at) * 1000))::bigint, \
                (trunc(extract(epoch from n.archived_at) * 1000))::bigint, \
                n.actor_kind, n.actor_id, coalesce(au.name, n.actor_label), \
                n.entity_type, n.entity_id \
         from users u \
         left join lateral ( \
             select * from notifications where user_id = u.id \
             order by created_at desc limit 50 \
         ) n on true \
         left join users au on au.id = (case when n.actor_kind = 'user' \
             and n.actor_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' \
             then n.actor_id::uuid end) \
         where u.id = $1::uuid \
         order by n.created_at desc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    let mut seq = 0i64;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        seq = r.try_get::<i64, _>(0)?;
        let Some(id) = r.try_get::<Option<String>, _>(1)? else {
            continue; // the lateral's no-row case: a person with an empty inbox
        };
        let kind: String = r.try_get(2)?;
        let ms = |i: usize| -> Result<Option<String>, sqlx::Error> {
            Ok(r.try_get::<Option<i64>, _>(i)?.map(epoch_ms_to_iso))
        };
        let actor_kind: Option<String> = r.try_get(13)?;
        let entity_type: Option<String> = r.try_get(16)?;
        out.push(json!({
            "id": id,
            "kind": kind,
            "title": r.try_get::<String, _>(3)?,
            "body": r.try_get::<String, _>(4)?,
            "href": r.try_get::<String, _>(5)?,
            "readAt": ms(6)?,
            "createdAt": ms(7)?,
            "seq": r.try_get::<Option<i64>, _>(8)?,
            // NULL is a legacy row, which reads as a banner (I-15).
            "alert": r.try_get::<Option<String>, _>(9)?.unwrap_or_else(|| "banner".into()),
            "class": notify_class_of(&kind),
            "category": category_of(&kind).as_str(),
            "count": r.try_get::<i32, _>(10)?,
            "updatedAt": ms(11)?,
            "archivedAt": ms(12)?,
            "actor": actor_kind.map(|k| json!({
                "kind": k,
                "id": r.try_get::<Option<String>, _>(14).ok().flatten(),
                "name": r.try_get::<Option<String>, _>(15).ok().flatten(),
            })),
            "entity": entity_type.map(|t| json!({
                "type": t,
                "id": r.try_get::<Option<String>, _>(17).ok().flatten(),
            })),
            "reason": kind_reason(&kind),
        }));
    }
    Ok((out, seq))
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

/// Unread rows of ONE kind — research's rail badge. Its clearing gesture is
/// the href mark-read the run's page fires on open, so the badge and the
/// bell's row for the same run clear together.
pub async fn unread_count_of_kind(
    pg: &PgPool,
    user_id: &str,
    kind: &str,
) -> Result<i32, sqlx::Error> {
    let n: (i32,) = sqlx::query_as(
        "select count(*)::int from notifications \
         where user_id = $1::uuid and kind = $2 and read_at is null",
    )
    .bind(user_id)
    .bind(kind)
    .fetch_one(pg)
    .await?;
    Ok(n.0)
}

/// Mark specific notifications read, or all of the user's when neither
/// selector is given — and when ids is EMPTY, which folds into the same
/// all-rows update. `href` is the surface's selector: a page that was opened
/// knows its own href, not the ids of whatever rows point at it, and marking
/// by href clears exactly those rows without touching anything else. ids win
/// over href when both arrive; an empty href folds into all, like empty ids.
///
/// Answers the ids it read — the caller publishes the read-sync event
/// (`publish_read`) with them, and only when there are any. Reading never
/// takes a seq: only alerting writes do (I-10).
pub async fn mark_notifications_read(
    pg: &PgPool,
    user_id: &str,
    ids: Option<&[String]>,
    href: Option<&str>,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = match (ids, href) {
        (Some(list), _) if !list.is_empty() => {
            sqlx::query_as(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and id = any($2::uuid[]) and read_at is null \
                 returning id::text",
            )
            .bind(user_id)
            .bind(list)
            .fetch_all(pg)
            .await?
        }
        (_, Some(href)) if !href.is_empty() => return mark_read_by_href(pg, user_id, href).await,
        _ => {
            sqlx::query_as(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and read_at is null returning id::text",
            )
            .bind(user_id)
            .fetch_all(pg)
            .await?
        }
    };
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Mark read every unread row of the person's that points at `href` — the
/// surface's selector, and the thread and channel sweeps' — and answer the
/// ids.
pub async fn mark_read_by_href(
    pg: &PgPool,
    user_id: &str,
    href: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "update notifications set read_at = now() \
         where user_id = $1::uuid and href = $2 and read_at is null returning id::text",
    )
    .bind(user_id)
    .bind(href)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Make the caller's own rows UNREAD again — the Home Inbox's undo of a read
/// today, a mark-unread in W2-H — and answer the ids reopened. Archived rows
/// are not reopened (Done is undone by unarchiving, and stays read). With
/// `read_at`, only rows read at that instant come back — to the millisecond,
/// the precision the wire carries it at (`read_at` itself is stored to the
/// microsecond, so an exact compare against a wire timestamp never matched)
/// — the undo's guard against a newer read it must not reverse.
///
/// THE OPEN-GROUP INDEX. A read row left `notifications_open_group_idx`, so
/// its group may have filed a NEWER open row since; putting the old one back
/// with its key would be two open rows for one (person, key), and the update
/// would fail with 23505. So a reopened row whose key another open row holds
/// — or a newer row reopened in the same call — gives the key up: it comes
/// back as an ungrouped row, and the group's next write coalesces into the
/// row that holds the key. The person's users row is locked first, as every
/// writer locks it, so no keyed row can be filed between the check and the
/// update. Every path that sets `read_at = null` goes through here.
pub async fn reopen_notifications(
    pg: &PgPool,
    user_id: &str,
    ids: &[String],
    read_at: Option<&str>,
) -> Result<Vec<String>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pg.begin().await?;
    sqlx::query("select 1 from users where id = $1::uuid for no key update")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let rows: Vec<(String,)> = sqlx::query_as(
        "with target as ( \
           select n.id, n.group_key, \
                  row_number() over (partition by n.group_key \
                                     order by n.created_at desc, n.id desc) as rn \
             from notifications n \
            where n.user_id = $1::uuid and n.id = any($2::uuid[]) \
              and n.read_at is not null and n.archived_at is null \
              and ($3::timestamptz is null \
                   or date_trunc('milliseconds', n.read_at) \
                      = date_trunc('milliseconds', $3::timestamptz)) \
         ) \
         update notifications n \
            set read_at = null, \
                group_key = case \
                  when t.group_key is null or t.rn > 1 then null \
                  when exists (select 1 from notifications o \
                                where o.user_id = $1::uuid and o.group_key = t.group_key \
                                  and o.read_at is null and o.archived_at is null) then null \
                  else t.group_key end \
           from target t \
          where n.id = t.id \
         returning n.id::text",
    )
    .bind(user_id)
    .bind(ids)
    .bind(read_at)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The person's unread count and their For-you unread count (Needs you +
/// Direct), archived rows excluded, from one read of the unread index.
pub async fn unread_counts(pg: &PgPool, user_id: &str) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "select count(*) filter (where archived_at is null), \
                count(*) filter (where archived_at is null and kind = any($2::text[])) \
         from notifications where user_id = $1::uuid and read_at is null",
    )
    .bind(user_id)
    .bind(for_you_kinds())
    .fetch_one(pg)
    .await
}

/// The read-sync event for `ids` (I-16): the counts after the read, the ids —
/// at most `READ_IDS_MAX`; past it no ids and `resync: true` — and whether it
/// was a mark-all.
pub fn read_event(unread: i64, for_you: i64, ids: &[String], all: bool) -> UserEvent {
    let over = ids.len() > READ_IDS_MAX;
    UserEvent::Notifications {
        unread,
        unread_for_you: Some(for_you),
        read_ids: (!over).then(|| ids.to_vec()),
        all: Some(all),
        resync: over.then_some(true),
    }
}

/// Tell the person's open pages and devices that these rows were READ, so
/// every bell drops and every client closes the OS banners it posted for them
/// (I-16). A failed count is logged and nothing is published: the poll is the
/// floor.
pub async fn publish_read(deps: &NotifyDeps, user_id: &str, ids: &[String], all: bool) {
    match unread_counts(&deps.pg, user_id).await {
        Ok((unread, for_you)) => publish_user(
            &deps.realtime,
            user_id,
            &read_event(unread, for_you, ids, all),
        ),
        Err(e) => tracing::error!("[notifications] unread count for read-sync failed: {e}"),
    }
}

/// Tell the person's open pages their unread count moved because something
/// was READ — `{"type":"notifications","unread":N}` on `user:<id>`. A
/// second tab's bell drops the moment the first one clears a row, instead of
/// on its next poll. A failed count is logged and nothing is published: the
/// poll is the floor.
///
/// Wave 1's call, kept for a caller that does not know WHICH rows were read:
/// the counts only, no `readIds` and no `all` (a client must not close
/// banners on a guess). Every read path in the tree publishes `publish_read`.
pub async fn publish_unread(deps: &NotifyDeps, user_id: &str) {
    match unread_counts(&deps.pg, user_id).await {
        Ok((unread, for_you)) => publish_user(
            &deps.realtime,
            user_id,
            &UserEvent::Notifications {
                unread,
                unread_for_you: Some(for_you),
                read_ids: None,
                all: None,
                resync: None,
            },
        ),
        Err(e) => tracing::error!("[notifications] unread count for read-sync failed: {e}"),
    }
}

/// Reading a channel or DM to its end clears the bell rows that point at it —
/// its DM row and any @mention in it — the shape `clear_thread_notifications`
/// gives agent threads. The caller decides "to its end" (the cursor covers
/// the newest message); this clears by the channel's href and answers the
/// ids it read.
pub async fn clear_channel_notifications(
    pg: &PgPool,
    user_id: &str,
    channel_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let href = format!("/comms/channel/{channel_id}");
    mark_read_by_href(pg, user_id, &href).await
}

/// The thing these rows are about resolved (an approval decided, an MCP
/// server back up): mark read every unread row about it, for EVERY recipient,
/// and publish the read-sync event to each. For producers only — no HTTP
/// route may reach it (C24); a person's own "clear" is `clear_entity`.
/// Answers how many rows it read.
pub async fn resolve_entity(
    deps: &NotifyDeps,
    entity: &NotifyEntity,
) -> Result<usize, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "update notifications set read_at = now() \
         where entity_type = $1 and entity_id = $2 and read_at is null \
         returning user_id::text, id::text",
    )
    .bind(entity.kind)
    .bind(&entity.id)
    .fetch_all(&deps.pg)
    .await?;
    let n = rows.len();
    let mut by_user: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (user, id) in rows {
        by_user.entry(user).or_default().push(id);
    }
    for (user, ids) in by_user {
        publish_read(deps, &user, &ids, false).await;
    }
    Ok(n)
}

/// `resolve_entity` for ONE person: their unread rows about the entity, and
/// only theirs (the future HTTP `entity` selector maps here). Publishes their
/// read-sync event when anything was read, and answers the ids.
pub async fn clear_entity(
    deps: &NotifyDeps,
    user_id: &str,
    entity: &NotifyEntity,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "update notifications set read_at = now() \
         where user_id = $1::uuid and entity_type = $2 and entity_id = $3 and read_at is null \
         returning id::text",
    )
    .bind(user_id)
    .bind(entity.kind)
    .bind(&entity.id)
    .fetch_all(&deps.pg)
    .await?;
    let ids: Vec<String> = rows.into_iter().map(|(id,)| id).collect();
    if !ids.is_empty() {
        publish_read(deps, user_id, &ids, false).await;
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_event_names_ids_up_to_the_cap() {
        let ids: Vec<String> = (0..3).map(|i| format!("id-{i}")).collect();
        let s = serde_json::to_string(&read_event(4, 1, &ids, false)).unwrap();
        assert_eq!(
            s,
            r#"{"type":"notifications","unread":4,"unreadForYou":1,"readIds":["id-0","id-1","id-2"],"all":false}"#
        );
        let many: Vec<String> = (0..=READ_IDS_MAX).map(|i| format!("id-{i}")).collect();
        let s = serde_json::to_string(&read_event(0, 0, &many, true)).unwrap();
        assert_eq!(
            s,
            r#"{"type":"notifications","unread":0,"unreadForYou":0,"all":true,"resync":true}"#
        );
        let exact: Vec<String> = (0..READ_IDS_MAX).map(|i| format!("id-{i}")).collect();
        let v = serde_json::to_value(read_event(0, 0, &exact, false)).unwrap();
        assert_eq!(v["readIds"].as_array().unwrap().len(), READ_IDS_MAX);
        assert!(v.get("resync").is_none());
    }
}
