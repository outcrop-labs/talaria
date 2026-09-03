// Live-DB proof of the mark-read selectors (cargo test -- --ignored). The
// href arm is an UPDATE whose matching only Postgres can confirm, and the
// precedence it promises — ids win over href, an empty href folds into all
// like empty ids — is the bell's clearing contract: a row marked read by the
// wrong arm is a notification that came back from the dead. House rule:
// #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test notifications_reads -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::notify::{mark_notifications_read, unread_count};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

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
    let pg = pool().await;
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
