// Live-DB proof of the per-thread read cursors (cargo test -- --ignored).
// A reply's seq comes from the channel's own counter, but the channel
// cursor advances only on the main feed — so the unread predicates need the
// thread's own row to say "this person read this far in this thread", and
// none of it is provable without Postgres evaluating the EXISTS the way it
// does. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test -p talaria-api --test it channel_thread_reads:: -- --ignored

use crate::support::{call_json, live_app_state, minted_member_with_email, person, pg};
use serde_json::json;
use sqlx::postgres::PgPool;
use talaria_api::channels::{
    channel_read_caught_up, channel_thread_read_caught_up, channel_unread_total, create_channel,
    list_channels, list_shared_with_person, mark_channel_read, mark_channel_thread_read,
};
use talaria_api::notify::{NotifyDeps, clear_channel_thread_notifications};
use talaria_channel_replies::{ChannelPost, notify_channel_post};
use talaria_mentions::MentionSender;

/// Delete every fixture person under one email prefix; the cascade takes
/// their channels, members, thread reads, messages and bell rows with them.
/// Each test owns its own prefix, because cargo runs the suite's tests
/// concurrently and a shared prefix collides on users_sub_key (the
/// conversation_reads suite's own split). The prefix carries no trailing dash
/// so the reader's own no-suffix address matches too.
async fn cleanup(pg: &PgPool, prefix: &str) {
    sqlx::query("delete from users where email like $1")
        .bind(format!("{prefix}%@test.invalid"))
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
    author: &str,
    status: &str,
    thread_root_id: Option<&str>,
) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into channel_messages \
             (channel_id, seq, author_type, author, content, status, thread_root_id) \
         values ($1::uuid, $2, 'user', $3, 'x', $4, $5::uuid) returning id::text",
    )
    .bind(channel)
    .bind(seq)
    .bind(author)
    .bind(status)
    .bind(thread_root_id)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn add_member(pg: &PgPool, channel: &str, user: &str) {
    sqlx::query("insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(channel)
        .bind(user)
        .execute(pg)
        .await
        .unwrap();
}

/// The channel's unread count as each of the three predicate spellings reads
/// it — the rail pill (`list_channels`), the rail badge
/// (`channel_unread_total`; the reader is in this one room only) and the
/// profile drawer's shared rooms (`shared_rooms_sql!`, viewed with `mate`).
/// Asserting they agree at every step is what holds the spellings together.
async fn unread(pg: &PgPool, reader: &str, mate: &str, channel: &str) -> i32 {
    let pill = list_channels(pg, reader)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == channel)
        .map(|row| row.unread_count)
        .expect("channel visible to its reader");
    let badge = channel_unread_total(pg, reader).await.unwrap();
    let drawer = list_shared_with_person(pg, reader, mate)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.id == channel)
        .map(|row| row.unread_count)
        .expect("channel shared with the mate");
    assert_eq!(
        (pill, badge, drawer),
        (pill, pill, pill),
        "the pill, the rail badge and the profile drawer must agree"
    );
    pill
}

/// One bell row, filed the way the producers file it: a `thread-reply` row
/// under `reply:<root>`, a `mention` with no group, the `all`-level activity
/// row under `channel:<id>:activity`. All on the channel's href.
async fn bell(pg: &PgPool, user: &str, channel: &str, kind: &str, group: Option<String>) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into notifications (user_id, kind, title, body, href, group_key, read_at) \
         values ($1::uuid, $2, 't', 'b', $3, $4, null) returning id::text",
    )
    .bind(user)
    .bind(kind)
    .bind(format!("/comms/channel/{channel}"))
    .bind(group)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn is_read(pg: &PgPool, id: &str) -> bool {
    sqlx::query_scalar("select read_at is not null from notifications where id = $1::uuid")
        .bind(id)
        .fetch_one(pg)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn unread_predicates_honor_the_thread_cursor() {
    let pg = pg().await;
    cleanup(&pg, "chanthr-reads").await;
    let reader = person(
        &pg,
        "chanthr-reads",
        "chanthr-reads@test.invalid",
        "Thread Reader",
    )
    .await;
    let mate_email = "chanthr-reads-mate@test.invalid";
    let mate = person(&pg, "chanthr-reads-mate", mate_email, "Thread Mate").await;
    let channel = create_channel(&pg, &reader, "chanthr-room", None, "room")
        .await
        .unwrap()
        .id;
    add_member(&pg, &channel, &mate).await;

    // A root in the main feed and a reply on it: with no cursor rows at all,
    // both count — the thread cursor row does not exist yet.
    let root = land(&pg, &channel, 1, mate_email, "complete", None).await;
    land(&pg, &channel, 2, mate_email, "complete", Some(&root)).await;
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 2);

    // Reading ONLY the thread clears the reply from all three counts, while
    // the root itself still waits.
    mark_channel_thread_read(&pg, &root, &reader, 2)
        .await
        .unwrap();
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 1);
    // Catch the root up: the channel cursor covers seq 1, and the whole
    // channel reads clean even though only the thread was "opened".
    mark_channel_read(&pg, &channel, &reader, 1).await.unwrap();
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 0);

    // A later main-flow message is NOT the thread's: the thread cursor does
    // not cover it, so it counts until the channel cursor catches up.
    land(&pg, &channel, 3, mate_email, "complete", None).await;
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 1);
    mark_channel_read(&pg, &channel, &reader, 3).await.unwrap();
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 0);

    // A newer reply waits past the thread cursor, and a stale cursor can
    // never pull it backwards: greatest() holds the line.
    land(&pg, &channel, 4, mate_email, "complete", Some(&root)).await;
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 1);
    mark_channel_thread_read(&pg, &root, &reader, 4)
        .await
        .unwrap();
    mark_channel_thread_read(&pg, &root, &reader, 1)
        .await
        .unwrap();
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 0);

    // One thread's cursor is not another's: a reply on a second thread
    // counts however far the first thread has been read.
    let other = land(&pg, &channel, 5, mate_email, "complete", None).await;
    land(&pg, &channel, 6, mate_email, "complete", Some(&other)).await;
    mark_channel_read(&pg, &channel, &reader, 5).await.unwrap();
    mark_channel_thread_read(&pg, &root, &reader, 99)
        .await
        .unwrap();
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 1);

    cleanup(&pg, "chanthr-reads").await;
}

/// The read route's gates read a thread read exactly as the badge does. The
/// channel gate is the one that changed: "the channel cursor covers the newest
/// complete message" could never pass from the main feed while the newest
/// message was a reply (the feed shows no replies), even with that reply read
/// in its panel. The badge read 0 and the bell kept the channel lit.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn caught_up_gates_read_a_thread_read_as_the_badge_does() {
    let pg = pg().await;
    cleanup(&pg, "chanthr-gates").await;
    let reader_email = "chanthr-gates@test.invalid";
    let reader = person(&pg, "chanthr-gates", reader_email, "Gate Reader").await;
    let mate_email = "chanthr-gates-mate@test.invalid";
    let mate = person(&pg, "chanthr-gates-mate", mate_email, "Gate Mate").await;
    let channel = create_channel(&pg, &reader, "chanthr-gates-room", None, "room")
        .await
        .unwrap()
        .id;
    add_member(&pg, &channel, &mate).await;

    // An empty channel and an empty thread are caught up.
    assert!(
        channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );
    let root = land(&pg, &channel, 1, reader_email, "complete", None).await;
    assert!(
        channel_thread_read_caught_up(&pg, &root, &reader)
            .await
            .unwrap()
    );

    // The feed's newest is seq 2; the newest message is the reply at 3.
    land(&pg, &channel, 2, mate_email, "complete", None).await;
    land(&pg, &channel, 3, mate_email, "complete", Some(&root)).await;
    mark_channel_read(&pg, &channel, &reader, 2).await.unwrap();
    assert!(
        !channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );
    assert!(
        !channel_thread_read_caught_up(&pg, &root, &reader)
            .await
            .unwrap()
    );
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 1);

    // Reading the thread catches up the thread AND the channel: the badge
    // reads 0, and so do both gates.
    mark_channel_thread_read(&pg, &root, &reader, 3)
        .await
        .unwrap();
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 0);
    assert!(
        channel_thread_read_caught_up(&pg, &root, &reader)
            .await
            .unwrap()
    );
    assert!(
        channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );

    // A reply still streaming holds neither gate (only complete messages
    // count, as on the badge); a completed newer one holds both.
    land(&pg, &channel, 4, mate_email, "streaming", Some(&root)).await;
    assert!(
        channel_thread_read_caught_up(&pg, &root, &reader)
            .await
            .unwrap()
    );
    assert!(
        channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );
    land(&pg, &channel, 5, mate_email, "complete", Some(&root)).await;
    assert!(
        !channel_thread_read_caught_up(&pg, &root, &reader)
            .await
            .unwrap()
    );
    assert!(
        !channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );

    // A main-feed message past the channel cursor holds the channel gate
    // however far the thread is read, and the thread gate does not see it.
    mark_channel_thread_read(&pg, &root, &reader, 5)
        .await
        .unwrap();
    land(&pg, &channel, 6, mate_email, "complete", None).await;
    assert!(
        channel_thread_read_caught_up(&pg, &root, &reader)
            .await
            .unwrap()
    );
    assert!(
        !channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );
    mark_channel_read(&pg, &channel, &reader, 6).await.unwrap();
    assert!(
        channel_read_caught_up(&pg, &channel, &reader)
            .await
            .unwrap()
    );

    cleanup(&pg, "chanthr-gates").await;
}

/// A follower who already read a reply in its thread is not rung for it. An
/// agent's reply is inserted `streaming`, with its seq, at the start; an open
/// thread panel posts the read while it streams; the fan-out runs only at
/// completion. Filing then would leave the bell lit beside a badge reading 0.
/// A follower whose cursor is behind still gets the row, and so does a
/// caller that does not say which message it is.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reply_read_in_its_thread_rings_no_one_who_watched_it() {
    let pg = pg().await;
    cleanup(&pg, "chanthr-watch").await;
    let ada_email = "chanthr-watch@test.invalid";
    let ada = person(&pg, "chanthr-watch", ada_email, "Watch Ada").await;
    let cy_email = "chanthr-watch-cy@test.invalid";
    let cy = person(&pg, "chanthr-watch-cy", cy_email, "Watch Cy").await;
    let channel = create_channel(&pg, &ada, "chanthr-watch-room", None, "room")
        .await
        .unwrap()
        .id;
    add_member(&pg, &channel, &cy).await;
    let href = format!("/comms/channel/{channel}");
    let thread_rows = |user: String| {
        let pg = pg.clone();
        let href = href.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select count(*) from notifications \
                 where user_id = $1::uuid and kind = 'thread-reply' and href = $2",
            )
            .bind(user)
            .bind(href)
            .fetch_one(&pg)
            .await
            .unwrap()
        }
    };

    // Ada's root and Cy's reply make both of them the thread's people. The
    // agent's reply lands at seq 3; Ada's panel read it while it streamed,
    // and Cy's cursor stops at their own reply.
    let root = land(&pg, &channel, 1, ada_email, "complete", None).await;
    land(&pg, &channel, 2, cy_email, "complete", Some(&root)).await;
    let reply = land(&pg, &channel, 3, "chanthr-agent", "complete", Some(&root)).await;
    mark_channel_thread_read(&pg, &root, &ada, 3).await.unwrap();
    mark_channel_thread_read(&pg, &root, &cy, 2).await.unwrap();

    let deps = NotifyDeps::publishing(pg.clone(), None);
    let agent = MentionSender {
        user_id: "",
        label: "Watch Agent",
        agent_model: Some("chanthr-agent"),
    };
    let post = |message_id| ChannelPost {
        channel_id: &channel,
        channel_name: "chanthr-watch-room",
        sender: agent,
        content: "the checklist is green",
        thread_root_id: Some(&root),
        message_id,
    };
    notify_channel_post(&deps, &post(Some(reply.as_str()))).await;
    assert_eq!(thread_rows(ada.clone()).await, 0, "Ada watched it land");
    assert_eq!(thread_rows(cy.clone()).await, 1, "Cy's cursor is behind");

    // Without the message, nothing says what was read: the row files.
    notify_channel_post(&deps, &post(None)).await;
    assert_eq!(thread_rows(ada.clone()).await, 1);

    cleanup(&pg, "chanthr-watch").await;
}

/// A thread read to its end clears that thread's OWN bell rows — and only
/// them: the group_key sweep, not the href sweep, because a @mention row on
/// the same channel shares the thread row's href, and so do another thread's
/// row and the channel's activity row.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn thread_read_clears_only_its_bell_rows() {
    let pg = pg().await;
    cleanup(&pg, "chanthr-bell").await;
    let reader_email = "chanthr-bell@test.invalid";
    let reader = person(&pg, "chanthr-bell", reader_email, "Thread Bell").await;
    let channel = create_channel(&pg, &reader, "chanthr-bell-room", None, "room")
        .await
        .unwrap()
        .id;
    let root = land(&pg, &channel, 1, reader_email, "complete", None).await;
    land(
        &pg,
        &channel,
        2,
        "chanthr-mate-2@test.invalid",
        "complete",
        Some(&root),
    )
    .await;
    let other = land(&pg, &channel, 3, reader_email, "complete", None).await;

    let thread_row = bell(
        &pg,
        &reader,
        &channel,
        "thread-reply",
        Some(format!("reply:{root}")),
    )
    .await;
    let other_row = bell(
        &pg,
        &reader,
        &channel,
        "thread-reply",
        Some(format!("reply:{other}")),
    )
    .await;
    let mention = bell(&pg, &reader, &channel, "mention", None).await;
    let activity = bell(
        &pg,
        &reader,
        &channel,
        "channel-message",
        Some(format!("channel:{channel}:activity")),
    )
    .await;

    mark_channel_thread_read(&pg, &root, &reader, 2)
        .await
        .unwrap();
    let cleared = clear_channel_thread_notifications(&pg, &reader, &root)
        .await
        .unwrap();
    assert_eq!(cleared, vec![thread_row.clone()]);
    assert!(is_read(&pg, &thread_row).await);
    for (row, what) in [
        (&other_row, "another thread's row"),
        (&mention, "the @mention on the same href"),
        (&activity, "the channel's activity row"),
    ] {
        assert!(!is_read(&pg, row).await, "{what} still waits");
    }

    // Idempotent: the panel's effect fires per message, and the second read
    // finds nothing left to clear.
    let cleared = clear_channel_thread_notifications(&pg, &reader, &root)
        .await
        .unwrap();
    assert_eq!(cleared.len(), 0);

    cleanup(&pg, "chanthr-bell").await;
}

/// The whole route, through the real router with a minted session, in the
/// order the app makes the calls: the feed reads first, then the thread
/// panel. A thread read that leaves the channel unread elsewhere clears only
/// its own row. The one that leaves the whole channel caught up clears the
/// channel's rows, the @mention the main feed already read included.
#[tokio::test]
#[ignore = "needs a live dev database and redis (DATABASE_URL, REDIS_URL)"]
async fn the_read_route_clears_what_the_badge_says_was_read() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    cleanup(&pg, "chanthr-route").await;
    let reader_email = "chanthr-route@test.invalid";
    let (reader, cookie) = minted_member_with_email(&state, "chanthr-route", reader_email).await;
    let mate_email = "chanthr-route-mate@test.invalid";
    let mate = person(&pg, "chanthr-route-mate", mate_email, "Route Mate").await;
    let channel = create_channel(&pg, &reader, "chanthr-route-room", None, "room")
        .await
        .unwrap()
        .id;
    add_member(&pg, &channel, &mate).await;
    let read = format!("/api/channels/{channel}/read");

    // The reader's root, the mate's @mention of them, a second root the
    // reader replied on; then the mate replies on both threads.
    let root = land(&pg, &channel, 1, reader_email, "complete", None).await;
    land(&pg, &channel, 2, mate_email, "complete", None).await;
    let other = land(&pg, &channel, 3, mate_email, "complete", None).await;
    land(&pg, &channel, 4, reader_email, "complete", Some(&other)).await;
    land(&pg, &channel, 5, mate_email, "complete", Some(&root)).await;
    land(&pg, &channel, 6, mate_email, "complete", Some(&other)).await;
    let mention = bell(&pg, &reader, &channel, "mention", None).await;
    let root_row = bell(
        &pg,
        &reader,
        &channel,
        "thread-reply",
        Some(format!("reply:{root}")),
    )
    .await;
    let other_row = bell(
        &pg,
        &reader,
        &channel,
        "thread-reply",
        Some(format!("reply:{other}")),
    )
    .await;
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 4);

    // The feed reads to its newest message (seq 3): the replies still wait,
    // so nothing clears from the bell.
    let (status, body) = call_json(
        &state,
        "POST",
        &read,
        Some(&cookie),
        Some(json!({ "seq": 3 })),
    )
    .await;
    assert_eq!((status.as_u16(), body["cleared"].as_u64()), (200, Some(0)));
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 2);

    // The first thread, with no seq (the server reads to its newest reply):
    // its own row clears; the mention and the other thread's row wait,
    // because the other thread is still unread.
    let (status, body) = call_json(
        &state,
        "POST",
        &read,
        Some(&cookie),
        Some(json!({ "threadRootId": root })),
    )
    .await;
    assert_eq!((status.as_u16(), body["cleared"].as_u64()), (200, Some(1)));
    assert!(is_read(&pg, &root_row).await);
    assert!(!is_read(&pg, &mention).await, "the @mention still waits");
    assert!(
        !is_read(&pg, &other_row).await,
        "the other thread still waits"
    );
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 1);

    // The second thread is the last unread thing on the channel: the badge
    // reads 0, and the channel's rows clear with it, the @mention included.
    let (status, body) = call_json(
        &state,
        "POST",
        &read,
        Some(&cookie),
        Some(json!({ "threadRootId": other, "seq": 6 })),
    )
    .await;
    assert_eq!((status.as_u16(), body["cleared"].as_u64()), (200, Some(2)));
    assert!(is_read(&pg, &other_row).await);
    assert!(is_read(&pg, &mention).await);
    assert_eq!(unread(&pg, &reader, &mate, &channel).await, 0);

    // The main feed's own read still clears: a new mention on the feed, read
    // to its seq, with every thread already read.
    land(&pg, &channel, 7, mate_email, "complete", None).await;
    let fresh = bell(&pg, &reader, &channel, "mention", None).await;
    let (status, body) = call_json(
        &state,
        "POST",
        &read,
        Some(&cookie),
        Some(json!({ "seq": 7 })),
    )
    .await;
    assert_eq!((status.as_u16(), body["cleared"].as_u64()), (200, Some(1)));
    assert!(is_read(&pg, &fresh).await);

    // A root from another channel is not this channel's thread.
    let elsewhere = create_channel(&pg, &reader, "chanthr-route-elsewhere", None, "room")
        .await
        .unwrap()
        .id;
    let stranger = land(&pg, &elsewhere, 1, mate_email, "complete", None).await;
    let (status, _) = call_json(
        &state,
        "POST",
        &read,
        Some(&cookie),
        Some(json!({ "threadRootId": stranger })),
    )
    .await;
    assert_eq!(status.as_u16(), 404);

    cleanup(&pg, "chanthr-route").await;
}
