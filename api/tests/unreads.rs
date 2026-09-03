// Live-DB proof of the rail badges' one read (cargo test -- --ignored).
// /api/unreads is a tokio::join! of five counters, and a badge that disagreed
// with the pills it summarizes would be worse than no badge — so this suite
// builds one person's whole waiting world (rooms, agent chats, plans,
// research notifications) and asserts each arm counts exactly what its
// surface's own predicate says is waiting. The conversations arm starts at
// seq 0, not 1: the floor is -1, and this is where that crossing the route's
// total is provable. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test unreads -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::channels::{channel_unread_total, create_channel};
use talaria_api::conversations::{conversation_unread_total, create_conversation};
use talaria_api::notify::{unread_count, unread_count_of_kind};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Two throwaway people — the reader whose badges we count, and a teammate to
/// author turns and own the shared plan. The cascade takes everything below.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'unreads-rail-%@test.invalid'")
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

/// Land a channel message directly with an explicit seq — this suite proves
/// the counting predicates, not the counter machinery that allocates seqs.
async fn room_msg(
    pg: &PgPool,
    channel: &str,
    seq: i32,
    author_type: &str,
    author: &str,
    status: &str,
) {
    sqlx::query(
        "insert into channel_messages (channel_id, seq, author_type, author, content, status) \
         values ($1::uuid, $2, $3, $4, 'x', $5)",
    )
    .bind(channel)
    .bind(seq)
    .bind(author_type)
    .bind(author)
    .bind(status)
    .execute(pg)
    .await
    .unwrap();
}

/// Same for conversations: direct rows, seq as given (they start at 0).
async fn turn(
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

/// File one notification row, read or not, under the given kind.
async fn notify(pg: &PgPool, user: &str, kind: &str, read: bool) {
    sqlx::query(
        "insert into notifications (user_id, kind, title, body, href, read_at) \
         values ($1::uuid, $2, 't', 'b', '/x', case when $3 then now() else null end)",
    )
    .bind(user)
    .bind(kind)
    .bind(read)
    .execute(pg)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn every_arm_counts_what_its_surface_calls_waiting() {
    let pg = pool().await;
    cleanup(&pg).await;
    let reader = person(
        &pg,
        "unreads-rail-reader",
        "unreads-rail-reader@test.invalid",
        "Rail Reader",
    )
    .await;
    let mate = person(
        &pg,
        "unreads-rail-mate",
        "unreads-rail-mate@test.invalid",
        "Rail Mate",
    )
    .await;

    // — Rooms: two the reader is in (one archived with an unread waiting —
    // archived is not waiting), one they are not in at all. "Own voice" is the
    // author's EMAIL — the predicate's coalesce takes email first.
    let room = create_channel(&pg, &reader, "unreads-rail-room", None, "room")
        .await
        .unwrap();
    room_msg(
        &pg,
        &room.id,
        1,
        "user",
        "unreads-rail-mate@test.invalid",
        "complete",
    )
    .await; // unread
    room_msg(
        &pg,
        &room.id,
        2,
        "user",
        "unreads-rail-reader@test.invalid",
        "complete",
    )
    .await; // own voice
    room_msg(
        &pg,
        &room.id,
        3,
        "user",
        "unreads-rail-mate@test.invalid",
        "streaming",
    )
    .await; // not landed yet
    let second = create_channel(&pg, &reader, "unreads-rail-second", None, "room")
        .await
        .unwrap();
    room_msg(&pg, &second.id, 1, "agent", "claude-test", "complete").await; // unread
    let gone = create_channel(&pg, &reader, "unreads-rail-gone", None, "room")
        .await
        .unwrap();
    room_msg(
        &pg,
        &gone.id,
        1,
        "user",
        "unreads-rail-mate@test.invalid",
        "complete",
    )
    .await; // unread but archived
    sqlx::query("update channels set archived_at = now() where id = $1::uuid")
        .bind(&gone.id)
        .execute(&pg)
        .await
        .unwrap();
    let foreign = create_channel(&pg, &mate, "unreads-rail-foreign", None, "room")
        .await
        .unwrap();
    room_msg(
        &pg,
        &foreign.id,
        1,
        "user",
        "unreads-rail-mate@test.invalid",
        "complete",
    )
    .await; // not a member

    // — Agent chats: the reader's own thread, never opened. Seq 0 is a real
    // landed turn (conversations number from 0) and must count — the floor is
    // -1, and the badge crossing this boundary is the point of the arm.
    let chat = create_conversation(&pg, &reader, "claude-test", "Unreads chat", "chat", None)
        .await
        .unwrap();
    turn(&pg, &chat, 0, "assistant", "complete", None).await; // unread, at seq 0
    turn(&pg, &chat, 1, "user", "complete", Some(&reader)).await; // own turn
    turn(&pg, &chat, 2, "assistant", "streaming", None).await; // not landed yet

    // — Plans: the mate's plan, the reader a member — a teammate's opening
    // turn AND the reply are both someone else's work in an unopened thread.
    let plan = create_conversation(&pg, &mate, "claude-test", "Unreads plan", "plan", None)
        .await
        .unwrap();
    sqlx::query(
        "insert into conversation_members (conversation_id, user_id) values ($1::uuid, $2::uuid)",
    )
    .bind(&plan)
    .bind(&reader)
    .execute(&pg)
    .await
    .unwrap();
    turn(&pg, &plan, 0, "user", "complete", Some(&mate)).await;
    turn(&pg, &plan, 1, "assistant", "complete", None).await;

    // — The bell: research rows feed their own arm AND the bell; other kinds
    // feed only the bell; read rows feed neither.
    notify(&pg, &reader, "research", false).await;
    notify(&pg, &reader, "research", false).await;
    notify(&pg, &reader, "research", true).await;
    notify(&pg, &reader, "dm", false).await;

    // The route's exact join, arms asserted one at a time so a wrong number
    // names its own surface.
    let (rooms, chats, plans, research, bell) = tokio::join!(
        channel_unread_total(&pg, &reader),
        conversation_unread_total(&pg, &reader, "chat"),
        conversation_unread_total(&pg, &reader, "plan"),
        unread_count_of_kind(&pg, &reader, "research"),
        unread_count(&pg, &reader),
    );
    // Rooms: the mate's landed message + the agent's — own voice, streaming,
    // archived, and non-member all excluded.
    assert_eq!(rooms.unwrap(), 2);
    // Chats: seq 0 counts (the -1 floor), the reader's own turn and the
    // still-streaming reply do not.
    assert_eq!(chats.unwrap(), 1);
    // Plans: both the teammate's turn and the reply wait for the member.
    assert_eq!(plans.unwrap(), 2);
    // Research: kind-filtered, read rows gone.
    assert_eq!(research.unwrap(), 2);
    // The bell: everything unread, every kind.
    assert_eq!(bell.unwrap(), 3);

    // Comms is rooms plus chats under one badge — the route's only arithmetic.
    assert_eq!(2 + 1, 3);

    // The mate, author of most of this noise, waits for nothing: their own
    // channel has them as creator but no unread, their plan's own opening
    // turn never counted for them, and no notification points at them.
    let (m_rooms, m_chats, m_plans, m_research) = tokio::join!(
        channel_unread_total(&pg, &mate),
        conversation_unread_total(&pg, &mate, "chat"),
        conversation_unread_total(&pg, &mate, "plan"),
        unread_count_of_kind(&pg, &mate, "research"),
    );
    assert_eq!(
        (
            m_rooms.unwrap(),
            m_chats.unwrap(),
            m_plans.unwrap(),
            m_research.unwrap()
        ),
        (0, 0, 1, 0)
    );

    cleanup(&pg).await;
}
