// Live-DB proof that a queued ticket's "behind" list never names a ticket on
// a board the reader cannot see (cargo test -- --ignored).
//
// One developer agent works two boards. A member of the first board, who is
// not on the second, reads what a ticket queued for that agent waits behind:
// the first board's ticket by its ref, the second board's as a count. The
// guarantee is `board_role` against real board_members rows, which no unit
// test can vouch for.
//
//   DATABASE_URL=postgres://… cargo test --test it work_wait_behind_live:: -- --ignored

use crate::support::pg;
use sqlx::postgres::PgPool;
use talaria_api::work_wait::behind;

const READER: &str = "behind-reader@link-test.invalid";
const OUTSIDER: &str = "behind-private-owner@link-test.invalid";
const AGENT: &str = "behind-live-test-engineering";

async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from runs where kind = 'work-session' and input->>'agentModel' = $1")
        .bind(AGENT)
        .execute(pg)
        .await
        .unwrap();
    // The cascade takes the boards, their tasks and members with the users.
    sqlx::query("delete from users where email = any($1)")
        .bind(vec![READER.to_string(), OUTSIDER.to_string()])
        .execute(pg)
        .await
        .unwrap();
}

async fn user(pg: &PgPool, email: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $1, 'Behind Test', 'member') \
         returning id::text",
    )
    .bind(email)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// A board owned by `owner`, with `owner` its only member, and one ticket
/// on it that `AGENT` has a live work session on.
async fn board_with_live_ticket(pg: &PgPool, owner: &str, name: &str, title: &str) {
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ($1, $2::uuid) returning id::text",
    )
    .bind(name)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board)
    .bind(owner)
    .execute(pg)
    .await
    .unwrap();
    // No ticket number: the label falls back to the title's head, the
    // spelling that leaks most.
    let (task,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, title, status, priority, created_by) \
         values ($1::uuid, $2, 'in_progress', 'medium', 'live-test') returning id::text",
    )
    .bind(&board)
    .bind(title)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into runs (id, kind, subject_type, subject_id, state, phase, input) \
         values (gen_random_uuid(), 'work-session', 'task', $1, 'running', '', \
                 jsonb_build_object('taskId', $1::text, 'agentModel', $2::text))",
    )
    .bind(&task)
    .bind(AGENT)
    .execute(pg)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_ticket_on_a_private_board_is_counted_never_named() {
    let pg = pg().await;
    cleanup(&pg).await;
    let reader = user(&pg, READER).await;
    let outsider = user(&pg, OUTSIDER).await;
    board_with_live_ticket(&pg, &reader, "behind live — eng", "Fix the webhook retry").await;
    board_with_live_ticket(
        &pg,
        &outsider,
        "behind live — private",
        "Acquire Foo Corp — term sheet",
    )
    .await;

    let seen = behind(&pg, &reader, AGENT).await;
    assert_eq!(seen.visible, vec!["Fix the webhook retry".to_string()]);
    assert_eq!(seen.hidden, 1, "the private board's ticket is a count");
    // And the other way round: the outsider sees theirs, not the reader's.
    let theirs = behind(&pg, &outsider, AGENT).await;
    assert_eq!(
        theirs.visible,
        vec!["Acquire Foo Corp — term sheet".to_string()]
    );
    assert_eq!(theirs.hidden, 1);

    cleanup(&pg).await;
}
