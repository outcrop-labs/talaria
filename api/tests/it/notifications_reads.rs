// Live-DB proof of the mark-read selectors (cargo test -- --ignored). The
// href arm is an UPDATE whose matching only Postgres can confirm, and the
// precedence it promises — ids win over href, an empty href folds into all
// like empty ids — is the bell's clearing contract: a row marked read by the
// wrong arm is a notification that came back from the dead. House rule:
// #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test it notifications_reads:: -- --ignored

use crate::support::pg;
use futures_util::FutureExt;
use serde_json::Value;
use sqlx::postgres::PgPool;
use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use talaria_api::notify::{
    Category, InboxQuery, InboxView, NotificationInput, NotifyDeps, NotifyEntity, NotifyMeta,
    Selector, add_notification_with, archive_notifications, clear_entity, entity,
    kinds_of_category, list_page, mark_notifications_read, mark_notifications_unread,
    resolve_entity, unarchive_notifications, unread_count, unread_counts,
};
/// One throwaway user; the cascade takes the notification rows with it.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email = 'notify-reads@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

/// File one unread row for the user under the given kind/href and return its
/// id — the direct insert, not add_notification, because this suite proves
/// the SELECT/UPDATE shapes, not the routing.
async fn file(pg: &PgPool, user: &str, kind: &str, href: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href) \
         values ($1::uuid, $2, 't', 'b', $3) returning id::text",
    )
    .bind(user)
    .bind(kind)
    .bind(href)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn is_read(pg: &PgPool, id: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "select read_at is not null \
                                   from notifications where id = $1::uuid",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn href_marks_only_what_points_there_and_ids_still_win() {
    let pg = pg().await;
    cleanup(&pg).await;
    let (user,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) \
         values ('notify-reads', 'notify-reads@test.invalid', 'Notify Reads', 'member') \
         returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();

    // Two rows pointing at the plan the person is about to open, one DM
    // elsewhere, one already read by hand.
    let plan_a = file(&pg, &user, "plan-share", "/plan/aaa").await;
    let plan_a2 = file(&pg, &user, "agent-reply", "/plan/aaa").await;
    let dm = file(&pg, &user, "dm", "/comms/dm/bbb").await;
    let done = file(&pg, &user, "dm", "/plan/aaa").await;
    sqlx::query("update notifications set read_at = now() where id = $1::uuid")
        .bind(&done)
        .execute(&pg)
        .await
        .unwrap();
    assert_eq!(unread_count(&pg, &user).await.unwrap(), 3);

    // The href arm clears both rows pointing at /plan/aaa — and only those.
    mark_notifications_read(&pg, &user, None, Some("/plan/aaa"))
        .await
        .unwrap();
    assert!(is_read(&pg, &plan_a).await);
    assert!(is_read(&pg, &plan_a2).await);
    assert!(!is_read(&pg, &dm).await);
    // The already-read row was not touched again (read_at still set; the
    // count is the observable).
    assert_eq!(unread_count(&pg, &user).await.unwrap(), 1);

    // ids win over href when both arrive: marking the DM by id while offering
    // an href that would have matched nothing left anyway.
    mark_notifications_read(
        &pg,
        &user,
        Some(std::slice::from_ref(&dm)),
        Some("/nowhere"),
    )
    .await
    .unwrap();
    assert!(is_read(&pg, &dm).await);
    assert_eq!(unread_count(&pg, &user).await.unwrap(), 0);

    // Empty ids fold past the ids arm into href's, and empty href folds into
    // all — the forgiving spine the route documents.
    let late = file(&pg, &user, "dm", "/comms/dm/bbb").await;
    mark_notifications_read(&pg, &user, Some(&[]), Some("/comms/dm/bbb"))
        .await
        .unwrap();
    assert!(is_read(&pg, &late).await);
    let last = file(&pg, &user, "research", "/research/ccc").await;
    mark_notifications_read(&pg, &user, Some(&[]), Some(""))
        .await
        .unwrap();
    assert!(is_read(&pg, &last).await);

    cleanup(&pg).await;
}

// ── The inbox API: views, keyset pages, filters, Done ────────────────────────

/// A throwaway member for the inbox suite, every class routed in-app.
async fn inbox_person(pg: &PgPool, tag: &str) -> String {
    sqlx::query("delete from users where email = $1")
        .bind(format!("notify-inbox-{tag}@test.invalid"))
        .execute(pg)
        .await
        .unwrap();
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role, notify_prefs) \
         values ($1, $2, $3, 'member', $4::jsonb) returning id::text",
    )
    .bind(format!("notify-inbox-{tag}"))
    .bind(format!("notify-inbox-{tag}@test.invalid"))
    .bind(format!("Inbox {tag}"))
    .bind(r#"{"mention":"in_app","dm":"in_app","approval_pending":"in_app","judge_escalation":"in_app","agent_blocked":"in_app","gap_reported":"in_app","work_complete":"in_app","workchain_turn":"in_app","workchain_paused":"in_app","digest":"off"}"#)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn drop_person(pg: &PgPool, id: &str) {
    sqlx::query("delete from users where id = $1::uuid")
        .bind(id)
        .execute(pg)
        .await
        .unwrap();
}

/// File through the real write path (no Redis: nothing fans out).
async fn filed(pg: &PgPool, user: &str, kind: &str, href: &str, meta: &NotifyMeta) -> String {
    let deps = NotifyDeps::publishing(pg.clone(), None);
    add_notification_with(
        &deps,
        user,
        &NotificationInput {
            kind,
            title: "t",
            body: Some("b"),
            href: Some(href),
        },
        meta,
    )
    .await
    .unwrap()
    .id
}

fn ids_of(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// Every page of a query, following `next` to the end.
async fn all_pages(pg: &PgPool, user: &str, q: &InboxQuery) -> (Vec<Vec<String>>, i64) {
    let mut q = q.clone();
    let mut pages = Vec::new();
    let seq = loop {
        let (rows, seq, next) = list_page(pg, user, &q).await.unwrap();
        pages.push(ids_of(&rows));
        match next {
            Some(c) => q.before = Some(c),
            None => break seq,
        }
        assert!(pages.len() < 50, "paging never ends");
    };
    (pages, seq)
}

async fn oracle(pg: &PgPool, user: &str, sql_where: &str) -> Vec<String> {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select id::text from notifications where user_id = $1::uuid and {sql_where} \
         order by created_at desc, id desc"
    )))
    .bind(user)
    .fetch_all(pg)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn views_page_by_keyset_and_filters_match_the_sql() {
    let pg = pg().await;
    let user = inbox_person(&pg, "pages").await;
    // 130 For-you rows (Needs you + Direct) and 25 Following ones, one an
    // unknown kind (the fallback class).
    let for_you = [
        "dm",
        "mention",
        "approval_pending",
        "agent-reply",
        "task-assigned",
    ];
    for i in 0..130 {
        filed(
            &pg,
            &user,
            for_you[i % for_you.len()],
            &format!("/x/{i}"),
            &Default::default(),
        )
        .await;
    }
    for i in 0..24 {
        let kind = if i % 2 == 0 {
            "research"
        } else {
            "task-status"
        };
        filed(&pg, &user, kind, &format!("/f/{i}"), &Default::default()).await;
    }
    filed(&pg, &user, "brand-new-kind", "/f/new", &Default::default()).await;
    // Every third For-you row read.
    sqlx::query(
        "update notifications set read_at = now() where id in ( \
           select id from notifications where user_id = $1::uuid and kind = any($2) \
           order by created_at limit 43)",
    )
    .bind(&user)
    .bind(&for_you[..])
    .execute(&pg)
    .await
    .unwrap();

    // For you pages 50 / 50 / 30, no duplicate and no gap.
    let q = InboxQuery {
        view: InboxView::ForYou,
        ..Default::default()
    };
    let (pages, _) = all_pages(&pg, &user, &q).await;
    let sizes: Vec<usize> = pages.iter().map(Vec::len).collect();
    assert_eq!(sizes, [50, 50, 30]);
    let flat: Vec<String> = pages.concat();
    assert_eq!(
        flat.iter().collect::<HashSet<_>>().len(),
        130,
        "no duplicate"
    );
    let want = oracle(
        &pg,
        &user,
        "archived_at is null and kind = any(array['dm','mention','approval_pending','agent-reply','task-assigned'])",
    )
    .await;
    assert_eq!(flat, want, "the pages are the oracle, in order");

    // Filters against the SQL.
    let q = InboxQuery {
        view: InboxView::All,
        unread_only: true,
        categories: Some(vec![Category::Direct]),
        limit: 100,
        ..Default::default()
    };
    let (pages, _) = all_pages(&pg, &user, &q).await;
    let direct: Vec<String> = kinds_of_category(Category::Direct)
        .iter()
        .map(|k| format!("'{k}'"))
        .collect();
    let want = oracle(
        &pg,
        &user,
        &format!(
            "archived_at is null and read_at is null and kind = any(array[{}])",
            direct.join(",")
        ),
    )
    .await;
    assert!(!want.is_empty());
    assert_eq!(pages.concat(), want, "category=direct&unread=1");
    let q = InboxQuery {
        view: InboxView::All,
        classes: Some(vec!["work_complete".into()]),
        ..Default::default()
    };
    let (pages, _) = all_pages(&pg, &user, &q).await;
    assert_eq!(
        pages.concat().len(),
        25,
        "the fallback class takes the unknown kind"
    );
    let q = InboxQuery {
        view: InboxView::All,
        kinds: Some(vec!["research".into()]),
        ..Default::default()
    };
    assert_eq!(all_pages(&pg, &user, &q).await.0.concat().len(), 12);
    let q = InboxQuery {
        view: InboxView::All,
        categories: Some(vec![Category::Following]),
        classes: Some(vec!["research_ready_never".into()]),
        ..Default::default()
    };
    assert!(
        all_pages(&pg, &user, &q).await.0.concat().is_empty(),
        "filters AND across parameters"
    );

    // No parameters: wave 1's newest 50, every row.
    let (rows, seq, next) = list_page(&pg, &user, &InboxQuery::default()).await.unwrap();
    assert_eq!(ids_of(&rows), oracle(&pg, &user, "true").await[..50]);
    assert!(next.is_some());
    assert_eq!(seq, 155);

    // Two rows in one millisecond never fall between pages.
    sqlx::query(
        "update notifications set created_at = date_trunc('milliseconds', now()) + interval '1 hour' \
                                                 + (row_number * interval '1 microsecond') \
         from (select id as rid, row_number() over (order by id) from notifications \
               where user_id = $1::uuid order by id limit 4) t where id = t.rid",
    )
    .bind(&user)
    .execute(&pg)
    .await
    .unwrap();
    let q = InboxQuery {
        limit: 1,
        ..Default::default()
    };
    let mut q1 = q.clone();
    let mut got = Vec::new();
    for _ in 0..6 {
        let (rows, _, next) = list_page(&pg, &user, &q1).await.unwrap();
        got.extend(ids_of(&rows));
        q1.before = next;
    }
    assert_eq!(got, oracle(&pg, &user, "true").await[..6]);
    drop_person(&pg, &user).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn done_marks_read_and_comes_back_and_unread_reopens() {
    let pg = pg().await;
    let user = inbox_person(&pg, "done").await;
    let mut rows = Vec::new();
    for i in 0..6 {
        rows.push(
            filed(
                &pg,
                &user,
                "dm",
                &format!("/comms/dm/{i}"),
                &Default::default(),
            )
            .await,
        );
    }
    assert_eq!(unread_counts(&pg, &user).await.unwrap(), (6, 6));
    // Done: three rows, one of them already read.
    mark_notifications_read(&pg, &user, Some(&rows[2..3]), None)
        .await
        .unwrap();
    let three = rows[0..3].to_vec();
    let (archived, newly_read) = archive_notifications(&pg, &user, &Selector::Ids(three.clone()))
        .await
        .unwrap();
    assert_eq!(
        archived.iter().collect::<HashSet<_>>(),
        three.iter().collect()
    );
    assert_eq!(
        newly_read.iter().collect::<HashSet<_>>(),
        rows[0..2].iter().collect(),
        "the already-read row is archived, not read again"
    );
    let unread_archived: i64 = sqlx::query_scalar(
        "select count(*) from notifications where id = any($1::uuid[]) \
         and (read_at is null or archived_at is null)",
    )
    .bind(&three)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(unread_archived, 0, "Done sets archived_at AND read_at");
    let all = InboxQuery {
        view: InboxView::All,
        ..Default::default()
    };
    let done = InboxQuery {
        view: InboxView::Done,
        ..Default::default()
    };
    let in_all = ids_of(&list_page(&pg, &user, &all).await.unwrap().0);
    let in_done = ids_of(&list_page(&pg, &user, &done).await.unwrap().0);
    assert!(three.iter().all(|id| !in_all.contains(id)));
    assert_eq!(
        in_done.iter().collect::<HashSet<_>>(),
        three.iter().collect()
    );
    assert_eq!(unread_counts(&pg, &user).await.unwrap(), (3, 3));
    // Archiving again changes nothing.
    assert!(
        archive_notifications(&pg, &user, &Selector::Ids(three.clone()))
            .await
            .unwrap()
            .0
            .is_empty()
    );

    // Back from Done: in All again, still read.
    let back = unarchive_notifications(&pg, &user, &Selector::Ids(vec![rows[0].clone()]))
        .await
        .unwrap();
    assert_eq!(back, vec![rows[0].clone()]);
    let in_all = ids_of(&list_page(&pg, &user, &all).await.unwrap().0);
    assert!(in_all.contains(&rows[0]));
    assert_eq!(unread_counts(&pg, &user).await.unwrap(), (3, 3));

    // Unread again — the badge counts it — and a Done row stays Done.
    let reopened = mark_notifications_unread(
        &pg,
        &user,
        &Selector::Ids(vec![rows[0].clone(), rows[1].clone()]),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened,
        vec![rows[0].clone()],
        "Done rows are not reopened"
    );
    assert_eq!(unread_counts(&pg, &user).await.unwrap(), (4, 4));
    assert_eq!(unread_count(&pg, &user).await.unwrap(), 4);
    assert!(
        mark_notifications_unread(&pg, &user, &Selector::All)
            .await
            .unwrap()
            .is_empty()
    );
    // By href.
    mark_notifications_read(&pg, &user, None, Some("/comms/dm/4"))
        .await
        .unwrap();
    assert_eq!(
        mark_notifications_unread(&pg, &user, &Selector::Href("/comms/dm/4".into()))
            .await
            .unwrap(),
        vec![rows[4].clone()]
    );

    // N3: a Done row that is somehow unread counts nowhere.
    sqlx::query("update notifications set read_at = null where id = $1::uuid")
        .bind(&rows[1])
        .execute(&pg)
        .await
        .unwrap();
    let (unread, for_you) = unread_counts(&pg, &user).await.unwrap();
    assert_eq!(unread_count(&pg, &user).await.unwrap() as i64, unread);
    assert_eq!((unread, for_you), (4, 4));
    // ... and comes back from Done without its group key.
    sqlx::query("update notifications set group_key = 'dm:test' where id = $1::uuid")
        .bind(&rows[1])
        .execute(&pg)
        .await
        .unwrap();
    unarchive_notifications(&pg, &user, &Selector::Ids(vec![rows[1].clone()]))
        .await
        .unwrap();
    let key: Option<String> =
        sqlx::query_scalar("select group_key from notifications where id = $1::uuid")
            .bind(&rows[1])
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(key, None);

    // Done for every row (no selector): the five not Done — rows 0 and 1
    // came back, rows 3, 4 and 5 never left.
    let (all_done, _) = archive_notifications(&pg, &user, &Selector::All)
        .await
        .unwrap();
    assert_eq!(all_done.len(), 5);
    assert_eq!(unread_counts(&pg, &user).await.unwrap(), (0, 0));
    drop_person(&pg, &user).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_entity_selector_reaches_only_the_callers_rows() {
    let pg = pg().await;
    let a = inbox_person(&pg, "ent-a").await;
    let b = inbox_person(&pg, "ent-b").await;
    let about = NotifyMeta {
        entity: Some(NotifyEntity::new(entity::TASK, "task-entity-1")),
        ..Default::default()
    };
    let a1 = filed(&pg, &a, "task-assigned", "/t/1", &about).await;
    let a2 = filed(&pg, &a, "mention", "/t/1", &about).await;
    let a3 = filed(&pg, &a, "mention", "/t/2", &Default::default()).await;
    let b1 = filed(&pg, &b, "task-assigned", "/t/1", &about).await;
    let deps = NotifyDeps::publishing(pg.clone(), None);
    let read = clear_entity(&deps, &a, about.entity.as_ref().unwrap())
        .await
        .unwrap();
    assert_eq!(
        read.iter().collect::<HashSet<_>>(),
        [&a1, &a2].into_iter().collect()
    );
    let unread_b: Option<i64> = sqlx::query_scalar(
        "select count(*) from notifications where id = $1::uuid and read_at is null",
    )
    .bind(&b1)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(unread_b, Some(1), "the other person's row is untouched");
    // Done and unread by entity are the caller's too.
    let sel = Selector::Entity(about.entity.clone().unwrap());
    let (archived, _) = archive_notifications(&pg, &b, &sel).await.unwrap();
    assert_eq!(archived, vec![b1.clone()]);
    let reopened = mark_notifications_unread(&pg, &a, &sel).await.unwrap();
    assert_eq!(
        reopened.iter().collect::<HashSet<_>>(),
        [&a1, &a2].into_iter().collect()
    );
    assert!(!reopened.contains(&a3));
    drop_person(&pg, &a).await;
    drop_person(&pg, &b).await;
}

// ── N3: what a read and a reopen do to a row's deliveries ───────────────────

/// The delivery columns a row is filed with, as W2-E's write leaves them: a
/// deferred push due in `push_s` seconds, a mail due in `mail_s` seconds
/// (negative: already due), `mailed` (sent 4 minutes ago) and the batcher's
/// `attempts`. None leaves the column null.
#[derive(Clone, Copy, Default)]
struct Due {
    push_s: Option<i64>,
    mail_s: Option<i64>,
    mailed: bool,
    attempts: i32,
}

fn push_in(s: i64) -> Due {
    Due {
        push_s: Some(s),
        ..Due::default()
    }
}

fn mail_in(s: i64) -> Due {
    Due {
        mail_s: Some(s),
        ..Due::default()
    }
}

/// One unread row with these delivery columns.
async fn with_deliveries(pg: &PgPool, user: &str, kind: &str, href: &str, due: Due) -> String {
    let Due {
        push_s,
        mail_s,
        mailed,
        attempts,
    } = due;
    sqlx::query_scalar(
        "insert into notifications \
           (user_id, kind, title, body, href, push_due_at, email_due_at, emailed_at, email_attempts) \
         values ($1::uuid, $2, 't', 'b', $3, now() + make_interval(secs => $4::bigint), \
                 now() + make_interval(secs => $5::bigint), \
                 case when $6::boolean then now() - interval '4 minutes' end, $7) \
         returning id::text",
    )
    .bind(user)
    .bind(kind)
    .bind(href)
    .bind(push_s)
    .bind(mail_s)
    .bind(mailed)
    .bind(attempts)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// A row's delivery columns, in seconds from now: (read, push due, pushed,
/// mail due, mailed, attempts).
#[derive(Debug, PartialEq)]
struct Deliveries {
    read: bool,
    push_due_s: Option<i64>,
    pushed: bool,
    mail_due_s: Option<i64>,
    mailed: bool,
    attempts: i32,
}

async fn deliveries(pg: &PgPool, id: &str) -> Deliveries {
    let (read, push_due_s, pushed, mail_due_s, mailed, attempts): (
        bool,
        Option<i64>,
        bool,
        Option<i64>,
        bool,
        i32,
    ) = sqlx::query_as(
        "select read_at is not null, \
                round(extract(epoch from push_due_at - now()))::bigint, pushed_at is not null, \
                round(extract(epoch from email_due_at - now()))::bigint, emailed_at is not null, \
                email_attempts \
           from notifications where id = $1::uuid",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .unwrap();
    Deliveries {
        read,
        push_due_s,
        pushed,
        mail_due_s,
        mailed,
        attempts,
    }
}

/// `want` seconds from now, give or take the test's own runtime — or the
/// engine's test delay when the process runs with it.
fn about(got: Option<i64>, want: i64) -> bool {
    let want = std::env::var("TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(want);
    got.is_some_and(|s| (want - 60..=want + 1).contains(&s))
}

/// N3 (the engine review): a row read inside its 60 s push deferral and then
/// marked unread was still pushed at its old due time, and a reopened row was
/// mailed at its old due time — within the minute when that time had passed.
/// A read now cancels the deferred push on every read path here, a reopen
/// never brings it back (whatever read the row), and a pending mail is
/// re-armed from the reopen at the person's frequency; a row already mailed,
/// or given up on, stays as it was.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_read_cancels_the_deferred_push_and_a_reopen_rearms_the_mail_from_now() {
    let pg = pg().await;
    let user = inbox_person(&pg, "n3").await;
    sqlx::query(
        "update users set notify_prefs = '{\"email\":{\"frequency\":\"15m\"}}'::jsonb, \
                timezone = 'America/New_York' where id = $1::uuid",
    )
    .bind(&user)
    .execute(&pg)
    .await
    .unwrap();
    let deps = NotifyDeps::publishing(pg.clone(), None);
    const MIN: i64 = 60;

    // Every read path in the inbox cancels a pending deferred push.
    let by_ids = with_deliveries(
        &pg,
        &user,
        "dm",
        "/n3/ids",
        Due {
            push_s: Some(60),
            mail_s: Some(15 * MIN),
            ..Due::default()
        },
    )
    .await;
    let by_href = with_deliveries(&pg, &user, "dm", "/n3/href", push_in(60)).await;
    let by_done = with_deliveries(&pg, &user, "dm", "/n3/done", push_in(60)).await;
    let about_task = NotifyEntity::new(entity::TASK, "n3-task");
    let by_entity = with_deliveries(&pg, &user, "mention", "/n3/entity", push_in(60)).await;
    let by_resolve = with_deliveries(&pg, &user, "mention", "/n3/resolve", push_in(60)).await;
    sqlx::query("update notifications set entity_type = $2, entity_id = $3 where id = $1::uuid")
        .bind(&by_entity)
        .bind(about_task.kind)
        .bind(&about_task.id)
        .execute(&pg)
        .await
        .unwrap();
    let about_mcp = NotifyEntity::new(entity::MCP_SERVER, "n3-mcp");
    sqlx::query("update notifications set entity_type = $2, entity_id = $3 where id = $1::uuid")
        .bind(&by_resolve)
        .bind(about_mcp.kind)
        .bind(&about_mcp.id)
        .execute(&pg)
        .await
        .unwrap();
    // A push that already went keeps its due time (the record of when).
    let pushed = with_deliveries(&pg, &user, "dm", "/n3/pushed", push_in(-10)).await;
    sqlx::query("update notifications set pushed_at = now() where id = $1::uuid")
        .bind(&pushed)
        .execute(&pg)
        .await
        .unwrap();
    mark_notifications_read(&pg, &user, Some(&[by_ids.clone(), pushed.clone()]), None)
        .await
        .unwrap();
    mark_notifications_read(&pg, &user, None, Some("/n3/href"))
        .await
        .unwrap();
    archive_notifications(&pg, &user, &Selector::Ids(vec![by_done.clone()]))
        .await
        .unwrap();
    clear_entity(&deps, &user, &about_task).await.unwrap();
    resolve_entity(&deps, &about_mcp).await.unwrap();
    for (path, id) in [
        ("ids", &by_ids),
        ("href", &by_href),
        ("done", &by_done),
        ("entity", &by_entity),
        ("resolve", &by_resolve),
    ] {
        let d = deliveries(&pg, id).await;
        assert!(
            d.read && d.push_due_s.is_none() && !d.pushed,
            "{path}: {d:?}"
        );
    }
    let d = deliveries(&pg, &pushed).await;
    assert!(
        d.read && d.pushed && d.push_due_s.is_some(),
        "pushed: {d:?}"
    );
    // The read leaves the mail's due time: it is how a reopen knows one was
    // pending (the batcher skips read rows).
    assert!(about(deliveries(&pg, &by_ids).await.mail_due_s, 15 * MIN));

    // Rows read by a path outside this file (the Home Inbox's mark-read, a
    // thread sweep, the email-only route) keep their push_due_at; and rows in
    // each mail state.
    let other_read = with_deliveries(&pg, &user, "mention", "/n3/other", push_in(40)).await;
    let mail_past = with_deliveries(&pg, &user, "dm", "/n3/past", mail_in(-5 * MIN)).await;
    let mail_ahead = with_deliveries(&pg, &user, "dm", "/n3/ahead", mail_in(2 * MIN)).await;
    let needs_you = with_deliveries(
        &pg,
        &user,
        "approval_pending",
        "/n3/approval",
        mail_in(-5 * MIN),
    )
    .await;
    let mailed = with_deliveries(
        &pg,
        &user,
        "dm",
        "/n3/mailed",
        Due {
            mail_s: Some(-5 * MIN),
            mailed: true,
            ..Due::default()
        },
    )
    .await;
    let gave_up = with_deliveries(
        &pg,
        &user,
        "dm",
        "/n3/gaveup",
        Due {
            attempts: 3,
            ..Due::default()
        },
    )
    .await;
    let no_mail = with_deliveries(&pg, &user, "dm", "/n3/none", Due::default()).await;
    let rest = [
        &other_read,
        &mail_past,
        &mail_ahead,
        &needs_you,
        &mailed,
        &gave_up,
        &no_mail,
    ];
    sqlx::query("update notifications set read_at = now() where id = any($1::uuid[])")
        .bind(rest.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        .execute(&pg)
        .await
        .unwrap();
    let before_mailed = deliveries(&pg, &mailed).await;

    let mut all: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
    all.extend([
        by_ids.clone(),
        by_href.clone(),
        by_entity.clone(),
        by_resolve.clone(),
        pushed.clone(),
    ]);
    let reopened = mark_notifications_unread(&pg, &user, &Selector::Ids(all.clone()))
        .await
        .unwrap();
    assert_eq!(
        reopened.iter().collect::<HashSet<_>>(),
        all.iter().collect::<HashSet<_>>()
    );
    for id in &all {
        let d = deliveries(&pg, id).await;
        assert!(!d.read, "{id} reopened: {d:?}");
        if *id == pushed {
            assert!(
                d.pushed && d.push_due_s.is_some(),
                "pushed stays pushed: {d:?}"
            );
        } else {
            assert!(
                d.push_due_s.is_none() && !d.pushed,
                "no push comes back: {d:?}"
            );
        }
    }
    // A pending mail is due from the reopen at the person's frequency (15m),
    // whether its old due time had passed or was still ahead.
    for (what, id) in [
        ("past", &mail_past),
        ("ahead", &mail_ahead),
        ("ids", &by_ids),
    ] {
        let d = deliveries(&pg, id).await;
        assert!(
            about(d.mail_due_s, 15 * MIN) && !d.mailed && d.attempts == 0,
            "{what}: {d:?}"
        );
    }
    // Needs you waits its own 15 minutes (the frequency is 1d below too).
    assert!(about(
        deliveries(&pg, &needs_you).await.mail_due_s,
        15 * MIN
    ));
    // Mailed stays mailed, with its due time; given up stays given up; a row
    // that never had a mail gets none.
    assert_eq!(
        deliveries(&pg, &mailed).await,
        Deliveries {
            read: false,
            ..before_mailed
        }
    );
    let d = deliveries(&pg, &gave_up).await;
    assert!(
        d.mail_due_s.is_none() && !d.mailed && d.attempts == 3,
        "{d:?}"
    );
    let d = deliveries(&pg, &no_mail).await;
    assert!(d.mail_due_s.is_none() && !d.mailed, "{d:?}");

    // 1d: the next 09:00 on the person's own clock; off: no mail.
    sqlx::query("update users set notify_prefs = '{\"email\":{\"frequency\":\"1d\"}}'::jsonb where id = $1::uuid")
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
    let daily = with_deliveries(&pg, &user, "dm", "/n3/daily", mail_in(-5 * MIN)).await;
    let daily_needs = with_deliveries(
        &pg,
        &user,
        "approval_pending",
        "/n3/daily-approval",
        mail_in(-5 * MIN),
    )
    .await;
    sqlx::query("update notifications set read_at = now() where id = any($1::uuid[])")
        .bind(vec![daily.clone(), daily_needs.clone()])
        .execute(&pg)
        .await
        .unwrap();
    mark_notifications_unread(
        &pg,
        &user,
        &Selector::Ids(vec![daily.clone(), daily_needs.clone()]),
    )
    .await
    .unwrap();
    let override_on =
        std::env::var("TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS").is_ok_and(|v| !v.trim().is_empty());
    if !override_on {
        let (local, ahead): (String, i64) = sqlx::query_as(
            "select to_char(email_due_at at time zone 'America/New_York', 'HH24:MI'), \
                    round(extract(epoch from email_due_at - now()))::bigint \
               from notifications where id = $1::uuid",
        )
        .bind(&daily)
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(local, "09:00");
        assert!((0..=24 * 60 * MIN).contains(&ahead), "{ahead}");
    }
    assert!(about(
        deliveries(&pg, &daily_needs).await.mail_due_s,
        15 * MIN
    ));
    sqlx::query("update users set notify_prefs = '{\"email\":{\"frequency\":\"off\"}}'::jsonb where id = $1::uuid")
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
    let off = with_deliveries(&pg, &user, "dm", "/n3/off", mail_in(-5 * MIN)).await;
    sqlx::query("update notifications set read_at = now() where id = $1::uuid")
        .bind(&off)
        .execute(&pg)
        .await
        .unwrap();
    mark_notifications_unread(&pg, &user, &Selector::Ids(vec![off.clone()]))
        .await
        .unwrap();
    assert_eq!(deliveries(&pg, &off).await.mail_due_s, None);
    drop_person(&pg, &user).await;
}

/// A reopen reads the zone a write reads (the wave-2 integration run): a
/// person with no zone of their own, in a workspace whose brief runs on Tokyo
/// time, gets a reopened daily mail re-armed for 09:00 Tokyo — W2-E's write
/// arms it there — not for 09:00 UTC. The instance's `brief_config` is set
/// aside and put back, pass or fail.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reopen_with_no_zone_rearms_the_daily_mail_on_the_workspace_clock() {
    let pg = pg().await;
    let saved: Option<Value> =
        sqlx::query_scalar("select value from app_settings where key = 'brief_config'")
            .fetch_optional(&pg)
            .await
            .unwrap();
    let set_brief = |value: Option<Value>| {
        let pg = pg.clone();
        async move {
            sqlx::query("delete from app_settings where key = 'brief_config'")
                .execute(&pg)
                .await
                .unwrap();
            if let Some(v) = value {
                sqlx::query("insert into app_settings (key, value) values ('brief_config', $1)")
                    .bind(v)
                    .execute(&pg)
                    .await
                    .unwrap();
            }
        }
    };
    set_brief(Some(serde_json::json!({ "timeZone": "Asia/Tokyo" }))).await;
    let outcome = AssertUnwindSafe(async {
        let user = inbox_person(&pg, "n3-zone").await;
        sqlx::query(
            "update users set notify_prefs = '{\"email\":{\"frequency\":\"1d\"}}'::jsonb, \
                    timezone = null where id = $1::uuid",
        )
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
        let daily = with_deliveries(&pg, &user, "dm", "/n3/zone", mail_in(-5 * 60)).await;
        sqlx::query("update notifications set read_at = now() where id = $1::uuid")
            .bind(&daily)
            .execute(&pg)
            .await
            .unwrap();
        mark_notifications_unread(&pg, &user, &Selector::Ids(vec![daily.clone()]))
            .await
            .unwrap();
        let override_on = std::env::var("TALARIA_TEST_NOTIFY_EMAIL_DELAY_SECS")
            .is_ok_and(|v| !v.trim().is_empty());
        if !override_on {
            let (tokyo, utc, ahead): (String, String, i64) = sqlx::query_as(
                "select to_char(email_due_at at time zone 'Asia/Tokyo', 'HH24:MI'), \
                        to_char(email_due_at at time zone 'UTC', 'HH24:MI'), \
                        round(extract(epoch from email_due_at - now()))::bigint \
                   from notifications where id = $1::uuid",
            )
            .bind(&daily)
            .fetch_one(&pg)
            .await
            .unwrap();
            assert_eq!(
                tokyo, "09:00",
                "the workspace's clock, not UTC's ({utc} UTC)"
            );
            assert!((0..=24 * 3_600).contains(&ahead), "{ahead}");
        }
        // A zone of their own still wins.
        sqlx::query("update users set timezone = 'America/New_York' where id = $1::uuid")
            .bind(&user)
            .execute(&pg)
            .await
            .unwrap();
        sqlx::query("update notifications set read_at = now() where id = $1::uuid")
            .bind(&daily)
            .execute(&pg)
            .await
            .unwrap();
        mark_notifications_unread(&pg, &user, &Selector::Ids(vec![daily.clone()]))
            .await
            .unwrap();
        if !override_on {
            let (local,): (String,) = sqlx::query_as(
                "select to_char(email_due_at at time zone 'America/New_York', 'HH24:MI') \
                   from notifications where id = $1::uuid",
            )
            .bind(&daily)
            .fetch_one(&pg)
            .await
            .unwrap();
            assert_eq!(local, "09:00");
        }
        drop_person(&pg, &user).await;
    })
    .catch_unwind()
    .await;
    set_brief(saved).await;
    if let Err(p) = outcome {
        std::panic::resume_unwind(p);
    }
}
