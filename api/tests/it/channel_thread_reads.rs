// Live-DB proof of the per-thread read cursors (cargo test -- --ignored).
// A reply's seq comes from the channel's own counter, but the channel
// cursor advances only on the main feed — so the unread predicates need the
// thread's own row to say "this person read this far in this thread", and
// none of it is provable without Postgres evaluating the EXISTS the way it
// does. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test it channel_thread_reads:: -- --ignored

use crate::support::{person, pg};
use sqlx::postgres::PgPool;
use talaria_api::channels::{
    channel_unread_total, create_channel, list_channels, mark_channel_read,
    mark_channel_thread_read,
};
use talaria_api::notify::clear_channel_thread_notifications;

/// The predicate test's people — reader plus a mate to author root and
/// replies. The cascade takes their channels, members, thread reads and
/// messages with them. `chanthr-reads%` (no dash) so the reader's own
/// no-suffix address matches too — a % alone would leave a stale reader row
/// behind and the re-run would die on users_sub_key.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email like 'chanthr-reads%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

/// The bell test's people — a DIFFERENT prefix than `cleanup`'s, because cargo
/// runs the suite's tests concurrently and a shared prefix collides on
/// users_sub_key (the conversation_reads suite's own split).
async fn cleanup_bell(pg: &PgPool) {
    sqlx::query("delete from users where email like 'chanthr-bell%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

/// Land a channel message directly with an explicit seq — this suite proves
/// the counting predicates, not the counter machinery that allocates seqs.
async fn land(
    pg: &PgPool,
    channel: &str,
    seq: i32,
    author_type: &str,
    author: &str,
    status: &str,
    thread_root_id: Option<&str>,
) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into channel_messages \
             (channel_id, seq, author_type, author, content, status, thread_root_id) \
         values ($1::uuid, $2, $3, $4, 'x', $5, $6::uuid) returning id::text",
    )
    .bind(channel)
    .bind(seq)
    .bind(author_type)
    .bind(author)
    .bind(status)
    .bind(thread_root_id)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn unread(pg: &PgPool, user: &str, channel: &str) -> i32 {
    list_channels(pg, user)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == channel)
        .map(|row| row.unread_count)
        .expect("channel visible to its reader")
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn unread_predicates_honor_the_thread_cursor() {
    let pg = pg().await;
    cleanup(&pg).await;
    let reader = person(&pg, "chanthr-reads", "chanthr-reads@test.invalid", "Thread Reader").await;
    let mate = person(&pg, "chanthr-reads-mate", "chanthr-reads-mate@test.invalid", "Thread Mate").await;
    let channel = create_channel(&pg, &reader, "chanthr-room", None, "room")
        .await
        .unwrap()
        .id;
    sqlx::query("insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&channel)
        .bind(&mate)
        .execute(&pg)
        .await
        .unwrap();

    // A root in the main feed and a reply on it: with no cursor rows at all,
    // both count — the thread cursor row does not exist yet.
    let mate_email = "chanthr-reads-mate@test.invalid";
    let root = land(&pg, &channel, 1, "user", mate_email, "complete", None).await;
    land(&pg, &channel, 2, "user", mate_email, "complete", Some(&root)).await;
    assert_eq!(unread(&pg, &reader, &channel).await, 2);
    assert_eq!(channel_unread_total(&pg, &reader).await.unwrap(), 2);

    // Reading ONLY the thread clears both counts of the reply — the rail
    // pill and the rail badge agree — while the root itself still waits.
    mark_channel_thread_read(&pg, &root, &reader, 2).await.unwrap();
    assert_eq!(unread(&pg, &reader, &channel).await, 1);
    assert_eq!(channel_unread_total(&pg, &reader).await.unwrap(), 1);
    // Catch the root up: the channel cursor covers seq 1, and the whole
    // channel reads clean even though only the thread was "opened".
    mark_channel_read(&pg, &channel, &reader, 1).await.unwrap();
    assert_eq!(unread(&pg, &reader, &channel).await, 0);
    assert_eq!(channel_unread_total(&pg, &reader).await.unwrap(), 0);

    // A later main-flow message is NOT the thread's: the thread cursor does
    // not cover it, so it counts until the channel cursor catches up.
    land(&pg, &channel, 3, "user", mate_email, "complete", None).await;
    assert_eq!(unread(&pg, &reader, &channel).await, 1);
    mark_channel_read(&pg, &channel, &reader, 3).await.unwrap();
    assert_eq!(unread(&pg, &reader, &channel).await, 0);

    // A newer reply waits past the thread cursor, and a stale cursor can
    // never pull it backwards: greatest() holds the line.
    land(&pg, &channel, 4, "user", mate_email, "complete", Some(&root)).await;
    assert_eq!(unread(&pg, &reader, &channel).await, 1);
    mark_channel_thread_read(&pg, &root, &reader, 4).await.unwrap();
    mark_channel_thread_read(&pg, &root, &reader, 1).await.unwrap();
    assert_eq!(unread(&pg, &reader, &channel).await, 0);
    assert_eq!(channel_unread_total(&pg, &reader).await.unwrap(), 0);

    cleanup(&pg).await;
}

/// A thread read to its end clears that thread's OWN bell rows — and only
/// them: the group_key sweep, not the href sweep, because a @mention row on
/// the same channel shares the thread row's href. Its people live under a
/// different fixture prefix than the test above — cargo runs the suite's
/// tests concurrently, and a shared prefix is a unique-key collision.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn thread_read_clears_only_its_bell_rows() {
    let pg = pg().await;
    cleanup_bell(&pg).await;
    let reader = person(&pg, "chanthr-bell", "chanthr-bell@test.invalid", "Thread Bell").await;
    let channel = create_channel(&pg, &reader, "chanthr-bell-room", None, "room")
        .await
        .unwrap()
        .id;
    let root = land(
        &pg,
        &channel,
        1,
        "user",
        "chanthr-bell@test.invalid",
        "complete",
        None,
    )
    .await;
    land(
        &pg,
        &channel,
        2,
        "user",
        "chanthr-mate-2@test.invalid",
        "complete",
        Some(&root),
    )
    .await;

    // What notify_channel_post files for an away thread participant: a
    // thread-reply row grouped under `reply:<root>` on the channel's href…
    let (thread_row,): (String,) = sqlx::query_as(
        "insert into notifications \
             (user_id, kind, title, body, href, group_key, read_at) \
         values ($1::uuid, 'thread-reply', 't', 'b', $2, $3, null) returning id::text",
    )
    .bind(&reader)
    .bind(format!("/comms/channel/{channel}"))
    .bind(format!("reply:{root}"))
    .fetch_one(&pg)
    .await
    .unwrap();
    // …and a @mention row on the SAME href, not the thread's group: an
    // unread mention on the main feed is not read because a reply was.
    sqlx::query(
        "insert into notifications (user_id, kind, title, body, href, read_at) \
         values ($1::uuid, 'channel-mention', 't', 'b', $2, null)",
    )
    .bind(&reader)
    .bind(format!("/comms/channel/{channel}"))
    .execute(&pg)
    .await
    .unwrap();

    // The thread's cursor covers its newest complete reply — the route's
    // caught-up gate — so the sweep clears exactly the thread's row.
    mark_channel_thread_read(&pg, &root, &reader, 2).await.unwrap();
    let cleared = clear_channel_thread_notifications(&pg, &reader, &root)
        .await
        .unwrap();
    assert_eq!(cleared, vec![thread_row.clone()]);
    let mention: i64 = sqlx::query_scalar(
        "select count(*) from notifications \
         where user_id = $1::uuid and kind = 'channel-mention' and read_at is null",
    )
    .bind(&reader)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(mention, 1, "the @mention row on the same href still waits");

    // Idempotent: the panel's effect fires per message, and the second read
    // finds nothing left to clear.
    let cleared = clear_channel_thread_notifications(&pg, &reader, &root)
        .await
        .unwrap();
    assert_eq!(cleared.len(), 0);

    cleanup_bell(&pg).await;
}