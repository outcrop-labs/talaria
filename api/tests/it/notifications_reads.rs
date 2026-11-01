// Live-DB proof of the mark-read selectors (cargo test -- --ignored). The
// href arm is an UPDATE whose matching only Postgres can confirm, and the
// precedence it promises — ids win over href, an empty href folds into all
// like empty ids — is the bell's clearing contract: a row marked read by the
// wrong arm is a notification that came back from the dead. House rule:
// #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test it notifications_reads:: -- --ignored

use crate::support::pg;
use serde_json::Value;
use sqlx::postgres::PgPool;
use std::collections::HashSet;
use talaria_api::notify::{
    Category, InboxQuery, InboxView, NotificationInput, NotifyDeps, NotifyEntity, NotifyMeta,
    Selector, add_notification_with, archive_notifications, clear_entity, entity,
    kinds_of_category, list_page, mark_notifications_read, mark_notifications_unread,
    unarchive_notifications, unread_count, unread_counts,
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
