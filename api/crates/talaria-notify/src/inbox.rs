// The person's inbox: the list (wave 1's rows and the v2 rows, I-11) and its
// views, filters and keyset pages, the `?since` read the trackers use (I-19),
// the replay a reconnecting stream sends, the unread counts, the writes —
// mark read, Done (archive) and back, mark unread — by ids / href / entity /
// all, the read-sync event that tells their other tabs and devices (I-16), and
// the entity clears producers call when the thing a row is about resolves.
// The wire is docs/notifications/live-and-inbox.md.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;
use crate::classes::{
    Category, FALLBACK_CLASS, category_of, for_you_kinds, kind_reason, kinds_of, kinds_of_category,
    known_kinds, notify_class_of,
};
use crate::meta::{NotifyEntity, entity};

/// The inbox's own feature flags (I-23): `?since` reads, the views / filters /
/// keyset pages, the stream's replay by seq, and Done (archive).
pub const FEATURES: &[&str] = &["since", "inbox-v2", "sse-replay", "archive"];

/// The most rows one page carries, and the page a request without a `limit`
/// gets (wave 1's newest 50).
pub const PAGE_MAX: i64 = 100;
pub const PAGE_DEFAULT: i64 = 50;

/// The most rows one `?since` read carries; past it `more` is true.
pub const SINCE_MAX: i64 = 200;

/// The most rows a reconnecting stream replays; past it the stream says
/// `resync complete: false` and the client reads `?since` itself.
pub const REPLAY_MAX: usize = 100;

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

/// Which rows a page reads (`GET /api/notifications?view=`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InboxView {
    /// No `view`: wave 1's list — every row, Done ones included, newest first.
    #[default]
    Legacy,
    /// Needs you + Direct, not Done.
    ForYou,
    /// Everything not Done.
    All,
    /// Done (archived), most recently archived first.
    Done,
}

impl InboxView {
    pub const NAMES: &'static [&'static str] = &["foryou", "all", "done"];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "foryou" => Some(InboxView::ForYou),
            "all" => Some(InboxView::All),
            "done" => Some(InboxView::Done),
            _ => None,
        }
    }
}

/// A keyset position: the page's order key (`created_at`, or `archived_at`
/// in Done) to the MICROSECOND — the column's own precision, so two rows in
/// one millisecond can never fall between pages — and the row id that breaks
/// a tie. The route carries it opaque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub at_us: i64,
    pub id: String,
}

/// One page's question. `Default` is wave 1's: the newest 50, every row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxQuery {
    pub view: InboxView,
    pub unread_only: bool,
    pub categories: Option<Vec<Category>>,
    /// Validated class ids.
    pub classes: Option<Vec<String>>,
    pub kinds: Option<Vec<String>>,
    pub limit: i64,
    pub before: Option<Cursor>,
}

impl Default for InboxQuery {
    fn default() -> Self {
        InboxQuery {
            view: InboxView::Legacy,
            unread_only: false,
            categories: None,
            classes: None,
            kinds: None,
            limit: PAGE_DEFAULT,
            before: None,
        }
    }
}

/// A category's wire name back to the category.
pub fn parse_category(s: &str) -> Option<Category> {
    [
        Category::NeedsYou,
        Category::Direct,
        Category::Following,
        Category::Workspace,
    ]
    .into_iter()
    .find(|c| c.as_str() == s)
}

/// The kinds a list of categories (or of classes) matches, and whether it
/// also takes every kind the registry does not name — those are filed under
/// the fallback class, a Following one (`classes::FALLBACK_CLASS`).
fn category_filter(cats: &[Category]) -> (Vec<&'static str>, bool) {
    let kinds = cats.iter().flat_map(|c| kinds_of_category(*c)).collect();
    (kinds, cats.contains(&Category::Following))
}

fn class_filter(classes: &[String]) -> (Vec<&'static str>, bool) {
    let kinds = classes.iter().flat_map(|c| kinds_of(c)).collect();
    (kinds, classes.iter().any(|c| c == FALLBACK_CLASS))
}

/// The outer select every list read shares: the person's seq (the snapshot's,
/// I-10) and the row in the v2 shape, columns 1-17 (`row_json`).
const ROW_COLS: &str = "u.notify_seq, n.id::text, n.kind, n.title, n.body, n.href, \
     (trunc(extract(epoch from n.read_at) * 1000))::bigint, \
     (trunc(extract(epoch from n.created_at) * 1000))::bigint, \
     n.seq, n.alert, n.count, \
     (trunc(extract(epoch from n.updated_at) * 1000))::bigint, \
     (trunc(extract(epoch from n.archived_at) * 1000))::bigint, \
     n.actor_kind, n.actor_id, coalesce(au.name, n.actor_label), \
     n.entity_type, n.entity_id";

/// The actor's display name, for a user actor whose id is a uuid.
const ACTOR_JOIN: &str = "left join users au on au.id = (case when n.actor_kind = 'user' \
     and n.actor_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' \
     then n.actor_id::uuid end)";

/// One row (columns 1-17 of `ROW_COLS`) in the v2 wire shape, or None for
/// the lateral's no-row case: a person with an empty page.
fn row_json(r: &sqlx::postgres::PgRow) -> Result<Option<Value>, sqlx::Error> {
    let Some(id) = r.try_get::<Option<String>, _>(1)? else {
        return Ok(None);
    };
    let kind: String = r.try_get(2)?;
    let ms = |i: usize| -> Result<Option<String>, sqlx::Error> {
        Ok(r.try_get::<Option<i64>, _>(i)?.map(epoch_ms_to_iso))
    };
    let actor_kind: Option<String> = r.try_get(13)?;
    let entity_type: Option<String> = r.try_get(16)?;
    Ok(Some(json!({
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
    })))
}

/// One page of the person's inbox in the v2 wire shape, the person's notify
/// seq, and the cursor of the next page (None at the end), from ONE
/// statement — one snapshot, so every row with a seq at or below the answer
/// had committed when the rows were read (I-10).
///
/// The page is keyset, not offset: `(order key, id) < cursor`, newest first,
/// so a row filed while someone pages never shifts the next page (it lands
/// above the first one) and a row archived meanwhile only leaves a gap that
/// was already read. The order key is `created_at` (a re-alert moves it up),
/// or `archived_at` in Done.
///
/// It is not a read THROUGH that seq (a Quiet coalesce keeps an older
/// createdAt, and a page stops at its limit): only a `?since` read with
/// `more: false` lets a client record `seq` as read through (I-20).
pub async fn list_page(
    pg: &PgPool,
    user_id: &str,
    q: &InboxQuery,
) -> Result<(Vec<Value>, i64, Option<Cursor>), sqlx::Error> {
    let (view_where, order_col) = match q.view {
        InboxView::Legacy => ("true", "created_at"),
        InboxView::ForYou => (
            "x.archived_at is null and x.kind = any($12::text[])",
            "created_at",
        ),
        InboxView::All => ("x.archived_at is null", "created_at"),
        InboxView::Done => ("x.archived_at is not null", "archived_at"),
    };
    let sql = format!(
        "select {ROW_COLS}, n.cursor_us \
         from users u \
         left join lateral ( \
             select x.*, (trunc(extract(epoch from x.{order_col}) * 1000000))::bigint as cursor_us \
             from notifications x \
             where x.user_id = u.id and {view_where} \
               and (not $2::bool or x.read_at is null) \
               and ($3::text[] is null or x.kind = any($3::text[]) \
                    or ($4::bool and x.kind <> all($5::text[]))) \
               and ($6::text[] is null or x.kind = any($6::text[]) \
                    or ($7::bool and x.kind <> all($5::text[]))) \
               and ($8::text[] is null or x.kind = any($8::text[])) \
               and ($9::bigint is null or (x.{order_col}, x.id) < \
                    ('epoch'::timestamptz + $9::bigint * interval '1 microsecond', $10::uuid)) \
             order by x.{order_col} desc, x.id desc \
             limit $11 \
         ) n on true \
         {ACTOR_JOIN} \
         where u.id = $1::uuid \
         order by n.{order_col} desc, n.id desc"
    );
    let (cat_kinds, cat_fallback) = match &q.categories {
        Some(c) => {
            let (k, f) = category_filter(c);
            (Some(k), f)
        }
        None => (None, false),
    };
    let (class_kinds, class_fallback) = match &q.classes {
        Some(c) => {
            let (k, f) = class_filter(c);
            (Some(k), f)
        }
        None => (None, false),
    };
    let limit = q.limit.clamp(1, PAGE_MAX);
    // AssertSqlSafe: the interpolations are this file's own constants (the
    // column lists, a view's predicate, its order column); every value the
    // caller sent rides a bind.
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(q.unread_only)
        .bind(cat_kinds)
        .bind(cat_fallback)
        .bind(known_kinds())
        .bind(class_kinds)
        .bind(class_fallback)
        .bind(q.kinds.as_deref())
        .bind(q.before.as_ref().map(|c| c.at_us))
        .bind(q.before.as_ref().map(|c| c.id.as_str()))
        .bind(limit + 1);
    // Only For you names $12: a bind with no placeholder is refused.
    if q.view == InboxView::ForYou {
        query = query.bind(for_you_kinds());
    }
    let rows = query.fetch_all(pg).await?;
    let mut seq = 0i64;
    let mut out = Vec::with_capacity(rows.len());
    let mut keys = Vec::with_capacity(rows.len());
    for r in &rows {
        seq = r.try_get::<i64, _>(0)?;
        if let Some(row) = row_json(r)? {
            keys.push(Cursor {
                at_us: r.try_get::<Option<i64>, _>(18)?.unwrap_or(0),
                id: row["id"].as_str().unwrap_or_default().to_string(),
            });
            out.push(row);
        }
    }
    let next = if out.len() as i64 > limit {
        out.truncate(limit as usize);
        keys.truncate(limit as usize);
        keys.pop()
    } else {
        None
    };
    Ok((out, seq, next))
}

/// The newest 50 rows, every row (wave 1's list) — `list_page` with no
/// parameters, without its cursor.
pub async fn list_inbox(pg: &PgPool, user_id: &str) -> Result<(Vec<Value>, i64), sqlx::Error> {
    let (rows, seq, _) = list_page(pg, user_id, &InboxQuery::default()).await?;
    Ok((rows, seq))
}

/// `GET /api/notifications?since=S` (I-19): every row with `seq > S`, in seq
/// order, at most `SINCE_MAX`, whether there are `more`, and the person's seq
/// — one statement, one snapshot. Read, unread and Done rows alike: the
/// question is "what happened after S", and the client decides what to show.
/// With `more: false` the client has read THROUGH the answered seq (I-20);
/// with `more: true` it reads again from the last row's seq.
pub async fn list_since(
    pg: &PgPool,
    user_id: &str,
    since: i64,
) -> Result<(Vec<Value>, i64, bool), sqlx::Error> {
    let sql = format!(
        "select {ROW_COLS} \
         from users u \
         left join lateral ( \
             select * from notifications \
             where user_id = u.id and seq > $2 \
             order by seq limit $3 \
         ) n on true \
         {ACTOR_JOIN} \
         where u.id = $1::uuid \
         order by n.seq"
    );
    // AssertSqlSafe: only this file's column constants are interpolated.
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(since)
        .bind(SINCE_MAX + 1)
        .fetch_all(pg)
        .await?;
    let mut seq = 0i64;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        seq = r.try_get::<i64, _>(0)?;
        if let Some(row) = row_json(r)? {
            out.push(row);
        }
    }
    let more = out.len() as i64 > SINCE_MAX;
    out.truncate(SINCE_MAX as usize);
    Ok((out, seq, more))
}

/// What a reconnecting stream replays (I-19): the person's seq, and the
/// `(id, seq)` of every row with `seq > since` in seq order — or, past
/// `REPLAY_MAX`, no rows and `complete: false` (the client reads `?since`).
/// One statement, so "every row at or below `seq`" is exactly what it read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub seq: i64,
    pub rows: Vec<(String, i64)>,
    pub complete: bool,
}

pub async fn replay(pg: &PgPool, user_id: &str, since: i64) -> Result<Replay, sqlx::Error> {
    let rows: Vec<(i64, Option<String>, Option<i64>)> = sqlx::query_as(
        "select u.notify_seq, n.id::text, n.seq \
         from users u \
         left join lateral ( \
             select id, seq from notifications \
             where user_id = u.id and seq > $2 \
             order by seq limit $3 \
         ) n on true \
         where u.id = $1::uuid \
         order by n.seq",
    )
    .bind(user_id)
    .bind(since)
    .bind(REPLAY_MAX as i64 + 1)
    .fetch_all(pg)
    .await?;
    let seq = rows.first().map_or(since, |r| r.0);
    let found: Vec<(String, i64)> = rows
        .into_iter()
        .filter_map(|(_, id, s)| Some((id?, s?)))
        .collect();
    if found.len() > REPLAY_MAX {
        return Ok(Replay {
            seq,
            rows: Vec::new(),
            complete: false,
        });
    }
    Ok(Replay {
        seq,
        rows: found,
        complete: true,
    })
}

/// The person's notify seq now — what a stream's resync names after the hub
/// reconnects. A person with no row answers None.
pub async fn current_seq(pg: &PgPool, user_id: &str) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("select notify_seq from users where id = $1::uuid")
        .bind(user_id)
        .fetch_optional(pg)
        .await
}

/// Every unread row the person has, Done ones excluded — the bell's `unread`
/// and `/api/unreads`' `notifications` arm, both "all unread" (O-10). The same
/// predicate `unread_counts` counts `unread` with, so the GET's `unread` and
/// `unreadForYou` and the read-sync event's counts never disagree (N3). Done
/// marks read, so a Done row is unread only if something cleared `read_at`
/// past the doors here.
pub async fn unread_count(pg: &PgPool, user_id: &str) -> Result<i32, sqlx::Error> {
    let n: (i32,) = sqlx::query_as(
        "select count(*)::int from notifications \
         where user_id = $1::uuid and read_at is null and archived_at is null",
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

/// Which of the person's own rows a write acts on (`PUT /api/notifications`):
/// all of them, these ids, every row pointing at an href, or every row about
/// an entity. Always the caller's rows — no selector reaches another person's
/// (C24: `resolve_entity` is for producers, never a route).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selector {
    All,
    Ids(Vec<String>),
    Href(String),
    Entity(NotifyEntity),
}

/// The entity types a person may select their own rows by — `meta::entity`.
pub const ENTITY_TYPES: &[&str] = &[
    entity::TASK,
    entity::CHANNEL,
    entity::CONVERSATION,
    entity::APPROVAL,
    entity::MCP_SERVER,
    entity::PLAN,
    entity::RESEARCH_RUN,
    entity::KB_DOC,
];

/// A wire entity type back to its `meta::entity` constant.
pub fn entity_type(s: &str) -> Option<&'static str> {
    ENTITY_TYPES.iter().copied().find(|t| *t == s)
}

impl Selector {
    /// The binds the shared WHERE reads: `($2 ids, $3 href, $4 type, $5 id)`.
    fn binds(&self) -> (Option<&[String]>, Option<&str>, Option<&str>, Option<&str>) {
        match self {
            Selector::All => (None, None, None, None),
            Selector::Ids(ids) => (Some(ids), None, None, None),
            Selector::Href(h) => (None, Some(h), None, None),
            Selector::Entity(e) => (None, None, Some(e.kind), Some(&e.id)),
        }
    }
}

/// The selector as a WHERE fragment over `$2 … $5` (`Selector::binds`).
const SELECTED: &str = "($2::uuid[] is null or id = any($2::uuid[])) \
     and ($3::text is null or href = $3) \
     and ($4::text is null or (entity_type = $4 and entity_id = $5))";

/// Done: archive the selected rows that are not Done yet — and mark them
/// read, because Done also marks read (O-11): every reader that counts or
/// lists unread rows (the bell, Home, the brief, the rail badges) drops them
/// with no change of its own. Answers (the ids archived, the ones of those
/// that were unread until now). Archiving never takes a seq (I-10).
pub async fn archive_notifications(
    pg: &PgPool,
    user_id: &str,
    sel: &Selector,
) -> Result<(Vec<String>, Vec<String>), sqlx::Error> {
    let (ids, href, etype, eid) = sel.binds();
    // `read_at = now()` after the update is exactly "it was unread": a row
    // read earlier keeps its earlier stamp.
    let sql = format!(
        "update notifications set archived_at = now(), read_at = coalesce(read_at, now()) \
         where user_id = $1::uuid and archived_at is null and {SELECTED} \
         returning id::text, read_at = now()"
    );
    // AssertSqlSafe: `SELECTED` is this file's constant; the selector rides
    // binds.
    let rows: Vec<(String, bool)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(ids)
        .bind(href)
        .bind(etype)
        .bind(eid)
        .fetch_all(pg)
        .await?;
    let newly_read = rows
        .iter()
        .filter(|(_, was_unread)| *was_unread)
        .map(|(id, _)| id.clone())
        .collect();
    Ok((rows.into_iter().map(|(id, _)| id).collect(), newly_read))
}

/// Back from Done: clear `archived_at` on the selected Done rows. They stay
/// read (Done marked them). A Done row that is somehow still unread comes
/// back without its group key, so it can never collide with a newer open row
/// of its group (`notifications_open_group_idx`). Answers the ids.
pub async fn unarchive_notifications(
    pg: &PgPool,
    user_id: &str,
    sel: &Selector,
) -> Result<Vec<String>, sqlx::Error> {
    let (ids, href, etype, eid) = sel.binds();
    let sql = format!(
        "update notifications set archived_at = null, \
                group_key = case when read_at is null then null else group_key end \
         where user_id = $1::uuid and archived_at is not null and {SELECTED} \
         returning id::text"
    );
    // AssertSqlSafe: `SELECTED` is this file's constant.
    let rows: Vec<(String,)> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(ids)
        .bind(href)
        .bind(etype)
        .bind(eid)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Mark the selected rows UNREAD again — only read rows that are not Done —
/// through the one reopen door (`reopen_notifications`), so a row whose group
/// has a newer open row comes back without its key. `Selector::All` reopens
/// nothing: "mark everything unread" is not a gesture anyone makes, and a
/// client that lost its selector must not resurrect the whole inbox. Answers
/// the ids reopened.
pub async fn mark_notifications_unread(
    pg: &PgPool,
    user_id: &str,
    sel: &Selector,
) -> Result<Vec<String>, sqlx::Error> {
    let ids: Vec<String> = match sel {
        Selector::All => return Ok(Vec::new()),
        Selector::Ids(ids) => ids.clone(),
        Selector::Href(_) | Selector::Entity(_) => {
            let (ids, href, etype, eid) = sel.binds();
            let sql = format!(
                "select id::text from notifications \
                 where user_id = $1::uuid and read_at is not null and archived_at is null \
                   and {SELECTED}"
            );
            // AssertSqlSafe: `SELECTED` is this file's constant.
            sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(user_id)
                .bind(ids)
                .bind(href)
                .bind(etype)
                .bind(eid)
                .fetch_all(pg)
                .await?
        }
    };
    reopen_notifications(pg, user_id, &ids, None).await
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
/// banners on a guess). Every read path in the tree publishes `publish_read`;
/// this is what the paths that make rows UNREAD or bring them back from Done
/// publish (mark unread, unarchive, the Home Inbox's undo): the counts moved
/// up, nothing was read, and every page refetches on the event.
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
