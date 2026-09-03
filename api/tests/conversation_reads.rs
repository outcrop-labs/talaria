// Live-DB proof of the conversation read cursors (cargo test -- --ignored).
// The unread subquery's WHERE is the whole feature — whose turns count, which
// statuses count, what a member with no cursor row yet sees — and none of it
// is provable without Postgres evaluating the LEFT JOIN's NULL the way it
// does. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test conversation_reads -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::conversations::{
    create_conversation, latest_message_seq, list_conversations, mark_conversation_read,
};
use talaria_api::notify::clear_thread_notifications;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Two throwaway people — an owner and a teammate to share a plan with. The
/// cascade takes their conversations, members, reads, and messages with them.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'conv-reads-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, sub: &str, email: &str, name: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') returning id::text",
    )
    .bind(sub)
    .bind(email)
    .bind(name)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// Land a message directly — this suite proves the counting predicate, not
/// the streaming machinery that fills these rows in production.
async fn land(
    pg: &PgPool,
    conversation: &str,
    seq: i32,
    role: &str,
    status: &str,
    author: Option<&str>,
) {
    sqlx::query(
        "insert into messages (conversation_id, seq, role, status, content, author_user_id) \
         values ($1::uuid, $2, $3, $4, 'x', $5::uuid)",
    )
    .bind(conversation)
    .bind(seq)
    .bind(role)
    .bind(status)
    .bind(author)
    .execute(pg)
    .await
    .unwrap();
}

async fn unread(pg: &PgPool, user: &str, kind: &str, conversation: &str) -> i32 {
    list_conversations(pg, user, kind)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == conversation)
        .map(|row| row.unread_count)
        .expect("conversation visible to its reader")
}

/// The bell test's own people — a DIFFERENT prefix than `cleanup`'s, because
/// cargo runs the suite's tests concurrently and a shared prefix collides on
/// users_sub_key.
async fn cleanup_bell(pg: &PgPool) {
    sqlx::query("delete from users where email like 'conv-bell-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn unreads_count_whose_turns_and_cursors_only_advance() {
    let pg = pool().await;
    cleanup(&pg).await;
    let owner = person(
        &pg,
        "conv-reads-owner",
        "conv-reads-owner@test.invalid",
        "Reads Owner",
    )
    .await;
    let mate = person(
        &pg,
        "conv-reads-mate",
        "conv-reads-mate@test.invalid",
        "Reads Mate",
    )
    .await;

    // — The owner's own chat thread, never opened: no cursor row exists, so
    // the coalesce arm decides — every landed message after seq 0 counts,
    // except the owner's own turn and anything still streaming.
    let chat = create_conversation(&pg, &owner, "claude-test", "Never opened", "chat", None)
        .await
        .unwrap();
    land(&pg, &chat, 0, "user", "complete", Some(&owner)).await;
    land(&pg, &chat, 1, "assistant", "streaming", None).await;
    land(&pg, &chat, 2, "assistant", "complete", None).await;
    assert_eq!(unread(&pg, &owner, "chat", &chat).await, 1);

    // Reading to the thread's latest clears it — the no-seq arm of the route
    // computes exactly this value.
    assert_eq!(latest_message_seq(&pg, &chat).await.unwrap(), 2);
    mark_conversation_read(&pg, &chat, &owner, 2).await.unwrap();
    assert_eq!(unread(&pg, &owner, "chat", &chat).await, 0);

    // A stale cursor can never pull backwards: greatest() holds the line, so
    // a late-arriving lower seq is a no-op and the new reply stays the only
    // unread.
    mark_conversation_read(&pg, &chat, &owner, 0).await.unwrap();
    land(&pg, &chat, 3, "assistant", "complete", None).await;
    assert_eq!(unread(&pg, &owner, "chat", &chat).await, 1);
    mark_conversation_read(
        &pg,
        &chat,
        &owner,
        latest_message_seq(&pg, &chat).await.unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(unread(&pg, &owner, "chat", &chat).await, 0);

    // — A shared plan: the mate owns it, the owner collaborates. From the
    // owner's chair BOTH the teammate's turn and the assistant's replies are
    // other people's work landing in a thread they haven't opened.
    let plan = create_conversation(&pg, &mate, "claude-test", "Shared plan", "plan", None)
        .await
        .unwrap();
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
    )
    .bind(&plan)
    .bind(&owner)
    .execute(&pg)
    .await
    .unwrap();
    land(&pg, &plan, 0, "user", "complete", Some(&mate)).await;
    land(&pg, &plan, 1, "assistant", "complete", None).await;
    assert_eq!(unread(&pg, &owner, "plan", &plan).await, 2);

    // The mate's own opening turn was never unread TO THE MATE — one arm
    // excludes it — but the assistant's reply still waits: 1, not 2.
    assert_eq!(unread(&pg, &mate, "plan", &plan).await, 1);

    // The member reads to latest; the pill clears for them and the owner
    // label survives on the collaborator's row.
    mark_conversation_read(&pg, &plan, &owner, 1).await.unwrap();
    assert_eq!(unread(&pg, &owner, "plan", &plan).await, 0);
    let row = list_conversations(&pg, &owner, "plan")
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == plan)
        .unwrap();
    assert_eq!(row.role, "collaborator");
    assert_eq!(row.owner_label.as_deref(), Some("Reads Mate"));

    cleanup(&pg).await;
}

/// A whole-thread read is the end of the bell rows pointing at the thread:
/// the sweep clears exactly those (the href the writer filed, this reader),
/// touches nothing else, and is idempotent. The ROUTES own the "cursor
/// covers latest" gate; this proves what the sweep itself does once called.
/// Its people live under a DIFFERENT fixture prefix than the test above —
/// cargo runs the suite's tests concurrently, and a shared prefix is a
/// unique-key collision between them.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn reading_the_thread_clears_its_bell_rows() {
    let pg = pool().await;
    cleanup_bell(&pg).await;
    let owner = person(
        &pg,
        "conv-bell-owner",
        "conv-bell-owner@test.invalid",
        "Bell Owner",
    )
    .await;

    let chat = create_conversation(&pg, &owner, "claude-test", "Bell thread", "chat", None)
        .await
        .unwrap();
    land(&pg, &chat, 0, "user", "complete", Some(&owner)).await;
    land(&pg, &chat, 1, "assistant", "complete", None).await;
    // What notify_agent_reply would have filed for the away reader: the
    // thread-canonical href, one row, unread.
    let (row,): (String,) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href, read_at) \
         values ($1::uuid, 'agent-reply', 'Claude replied', 'x', $2, null) returning id::text",
    )
    .bind(&owner)
    .bind(format!("/comms/agent/claude-test/{chat}"))
    .fetch_one(&pg)
    .await
    .unwrap();
    // A row for ANOTHER place — the sweep must leave it waiting.
    sqlx::query(
        "insert into notifications (user_id, kind, title, body, href, read_at) \
         values ($1::uuid, 'agent-reply', 'Claude replied', 'x', '/plan/somewhere-else', null)",
    )
    .bind(&owner)
    .execute(&pg)
    .await
    .unwrap();

    let cleared = clear_thread_notifications(&pg, &owner, &chat)
        .await
        .unwrap();
    assert_eq!(cleared, 1);
    let read_at: Option<String> =
        sqlx::query_scalar("select read_at::text from notifications where id = $1::uuid")
            .bind(&row)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert!(read_at.is_some(), "the thread's own bell row is read");
    let other: i64 = sqlx::query_scalar(
        "select count(*) from notifications \
         where user_id = $1::uuid and read_at is null",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(other, 1, "the other thread's row still waits");

    // Idempotent: the second read (the stream keeps advancing the cursor)
    // finds nothing left to clear.
    let cleared = clear_thread_notifications(&pg, &owner, &chat)
        .await
        .unwrap();
    assert_eq!(cleared, 0);

    cleanup_bell(&pg).await;
}
