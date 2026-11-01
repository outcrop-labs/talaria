// The person's inbox: the list, the unread counts, mark-read by ids / href /
// all, and the read-sync event that tells their other tabs.

use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;

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
/// Answers how many rows it read — the caller publishes the read-sync event
/// (`publish_unread`) only when that is more than zero.
pub async fn mark_notifications_read(
    pg: &PgPool,
    user_id: &str,
    ids: Option<&[String]>,
    href: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let res = match (ids, href) {
        (Some(list), _) if !list.is_empty() => {
            sqlx::query(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and id = any($2::uuid[]) and read_at is null",
            )
            .bind(user_id)
            .bind(list)
            .execute(pg)
            .await?
        }
        (_, Some(href)) if !href.is_empty() => {
            sqlx::query(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and href = $2 and read_at is null",
            )
            .bind(user_id)
            .bind(href)
            .execute(pg)
            .await?
        }
        _ => {
            sqlx::query(
                "update notifications set read_at = now() \
                 where user_id = $1::uuid and read_at is null",
            )
            .bind(user_id)
            .execute(pg)
            .await?
        }
    };
    Ok(res.rows_affected())
}

/// Tell the person's open pages their unread count moved because something
/// was READ — `{"type":"notifications","unread":N}` on `user:<id>`. A
/// second tab's bell drops the moment the first one clears a row, instead of
/// on its next poll. A failed count is logged and nothing is published: the
/// poll is the floor.
pub async fn publish_unread(deps: &NotifyDeps, user_id: &str) {
    match unread_count(&deps.pg, user_id).await {
        Ok(n) => publish_user(
            &deps.realtime,
            user_id,
            &UserEvent::Notifications {
                unread: i64::from(n),
            },
        ),
        Err(e) => tracing::error!("[notifications] unread count for read-sync failed: {e}"),
    }
}

/// Reading a channel or DM to its end clears the bell rows that point at it —
/// its DM row and any @mention in it — the shape `clear_thread_notifications`
/// gives agent threads. The caller decides "to its end" (the cursor covers
/// the newest message); this clears by the channel's href and answers how
/// many rows it read.
pub async fn clear_channel_notifications(
    pg: &PgPool,
    user_id: &str,
    channel_id: &str,
) -> Result<u64, sqlx::Error> {
    let href = format!("/comms/channel/{channel_id}");
    mark_notifications_read(pg, user_id, None, Some(&href)).await
}
