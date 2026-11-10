// Live-DB proof that a reply nobody writes never holds its thread (release
// sweep chat-1). A restart in the middle of a reply left the row `streaming`
// with no writer: the next send queued behind it and was never answered, for
// the fifteen minutes the old sweep waited out. The lease arithmetic and the
// guarded close are WHERE clauses only Postgres can confirm, so these are
// `#[ignore]`d and CI's live-tests job runs them
// (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-conversations --lib -- --ignored orphan_live
//
// The tests run side by side in one process, and one of them sweeps every
// orphan in the database, so each asserts only on its own rows, and on their
// end state rather than on who closed them.

use serde_json::{Value, json};
use sqlx::PgPool;

use super::*;

pub(super) async fn pg() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// One throwaway person per test, keyed by its tag, swept before and after.
/// The cascade takes their conversations and messages with them.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub = $1")
        .bind(format!("orphan-live-{tag}"))
        .execute(pg)
        .await
        .unwrap();
}

async fn thread(pg: &PgPool, tag: &str) -> String {
    cleanup(pg, tag).await;
    let (user,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, 'Orphan Live', 'member') \
         returning id::text",
    )
    .bind(format!("orphan-live-{tag}"))
    .bind(format!("orphan-live-{tag}@test.invalid"))
    .fetch_one(pg)
    .await
    .unwrap();
    let conv = create_conversation(pg, &user, "orphan-live-agent", "orphan live", "chat", None)
        .await
        .unwrap();
    insert_user_message(
        pg,
        &conv,
        0,
        "tell me a long story",
        &json!([]),
        None,
        &json!({}),
    )
    .await
    .unwrap();
    conv
}

/// A `streaming` reply with `prose` in it, last stamped `quiet_secs` ago.
async fn reply(pg: &PgPool, conv: &str, seq: i32, prose: &str, quiet_secs: f64) -> String {
    let id = insert_streaming_assistant(pg, conv, seq, &json!({}))
        .await
        .unwrap();
    sqlx::query(
        "update messages set content = $2, streamed_at = now() - make_interval(secs => $3) \
         where id = $1::uuid",
    )
    .bind(&id)
    .bind(prose)
    .bind(quiet_secs)
    .execute(pg)
    .await
    .unwrap();
    id
}

async fn row(pg: &PgPool, id: &str) -> (String, String, Value) {
    sqlx::query_as("select status, content, metadata from messages where id = $1::uuid")
        .bind(id)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// The chat-1 thread: a reply cut off three minutes ago by a restart, with
/// the words it had. The next send must not queue behind it, and the row
/// keeps its words with the reason after them. The old sweep held the thread
/// for fifteen minutes of silence and answered `Some`, so the send queued.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reply_nobody_writes_does_not_hold_its_thread() {
    let pg = pg().await;
    let conv = thread(&pg, "cut-off").await;
    let id = reply(
        &pg,
        &conv,
        1,
        "The lamp at Garrick Point had been lit for forty",
        180.0,
    )
    .await;

    assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
    let (status, content, metadata) = row(&pg, &id).await;
    assert_eq!(status, "error");
    assert!(
        content.starts_with("The lamp at Garrick Point had been lit for forty\n\n(agent error: "),
        "{content}"
    );
    assert!(
        content.ends_with("most likely because Talaria restarted)"),
        "{content}"
    );
    assert_eq!(metadata["orphaned"], json!(true));
    // Asked again, nothing is in flight and nothing changes.
    assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
    assert_eq!(row(&pg, &id).await.1, content);
    cleanup(&pg, "cut-off").await;
}

/// An orphan that had said nothing yet is the reason alone.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_empty_orphan_is_the_reason_alone() {
    let pg = pg().await;
    let conv = thread(&pg, "empty").await;
    let id = reply(&pg, &conv, 1, "  \n", 180.0).await;

    assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
    let (status, content, _) = row(&pg, &id).await;
    assert_eq!(status, "error");
    assert!(content.starts_with("(agent error: "), "{content:?}");
    assert!(!content.contains('\n'), "{content:?}");
    cleanup(&pg, "empty").await;
}

/// A reply stamped inside the lease is someone's: another process's (a
/// deploy's overlap) or this one's. The send queues behind it.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_reply_stamped_inside_the_lease_is_in_flight() {
    let pg = pg().await;
    let conv = thread(&pg, "fresh").await;
    let id = reply(&pg, &conv, 1, "still going", 30.0).await;

    assert_eq!(
        active_streaming_assistant(&pg, &conv).await.unwrap(),
        Some(id.clone())
    );
    assert_eq!(row(&pg, &id).await.0, "streaming");
    cleanup(&pg, "fresh").await;
}

/// The writer's side: the hold, the beat, the sweep, and the fate a queued
/// send reads.
mod lease {
    use super::*;

    /// A reply this process holds, as a send makes one: held while it is
    /// fresh, then left quiet for `quiet_secs`. Made stale first and held
    /// after, a sibling test's sweep could take it in between.
    async fn held_reply(
        pg: &PgPool,
        conv: &str,
        prose: &str,
        quiet_secs: f64,
    ) -> (String, ReplyWriter) {
        let id = reply(pg, conv, 1, prose, 0.0).await;
        let writer = ReplyWriter::hold(&id);
        sqlx::query(
            "update messages set streamed_at = now() - make_interval(secs => $2) \
             where id = $1::uuid",
        )
        .bind(&id)
        .bind(quiet_secs)
        .execute(pg)
        .await
        .unwrap();
        (id, writer)
    }

    /// A reply this process holds is in flight however long it has been
    /// quiet (a turn waiting on the agent's headers, a long tool call), and
    /// the sweep never takes it. Once the hold goes, it is an orphan.
    #[tokio::test]
    #[ignore = "needs a live dev database (DATABASE_URL)"]
    async fn a_reply_this_process_writes_is_never_its_orphan() {
        let pg = pg().await;
        let conv = thread(&pg, "held").await;
        let (id, writer) = held_reply(&pg, &conv, "thinking", 20.0 * 60.0).await;
        assert_eq!(
            active_streaming_assistant(&pg, &conv).await.unwrap(),
            Some(id.clone())
        );
        let closed = sweep_orphaned_replies(&pg).await.unwrap();
        assert!(!closed.iter().any(|(m, _)| m == &id));
        assert_eq!(row(&pg, &id).await.0, "streaming");
        // The beat stamps it alive.
        assert!(beat_replies(&pg, &replies_written_here()).await.unwrap() >= 1);
        let (fresh,): (bool,) = sqlx::query_as(
            "select streamed_at > now() - interval '5 seconds' from messages where id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(&pg)
        .await
        .unwrap();
        assert!(fresh);

        drop(writer);
        sqlx::query(
            "update messages set streamed_at = now() - interval '3 minutes' where id = $1::uuid",
        )
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
        assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
        assert_eq!(row(&pg, &id).await.0, "error");
        cleanup(&pg, "held").await;
    }

    /// A sweep already under way when a reply is held, which reaches the
    /// database only after the reply has gone quiet, leaves it alone. The
    /// sweep read the holds before its statement ran: a hold in between was
    /// in neither, and the reply was closed under its writer. That is how a
    /// sibling test's sweep took the held rows above, one CI run in a few
    /// dozen. Here the sweep's pool has one connection, checked out, so the
    /// sweep starts and then waits while the reply is held and goes quiet.
    #[tokio::test]
    #[ignore = "needs a live dev database (DATABASE_URL)"]
    async fn a_sweep_under_way_when_a_reply_is_held_leaves_it_alone() {
        let pg = pg().await;
        let conv = thread(&pg, "under-way").await;
        let id = reply(&pg, &conv, 1, "thinking", 0.0).await;
        let url = std::env::var("DATABASE_URL").unwrap();
        let one = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let busy = one.acquire().await.unwrap();
        let mut sweep = std::pin::pin!(sweep_orphaned_replies(&one));
        let started = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(std::future::Future::poll(sweep.as_mut(), cx).is_pending())
        })
        .await;
        assert!(started, "the sweep waits for its pool's one connection");

        let writer = ReplyWriter::hold(&id);
        sqlx::query(
            "update messages set streamed_at = now() - interval '20 minutes' where id = $1::uuid",
        )
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
        drop(busy);
        let closed = sweep.await.unwrap();
        assert!(!closed.iter().any(|(m, _)| m == &id));
        assert_eq!(row(&pg, &id).await.0, "streaming");
        drop(writer);
        cleanup(&pg, "under-way").await;
    }

    /// The boot sweep: every orphan in the database, whatever thread it is
    /// in, and only orphans.
    #[tokio::test]
    #[ignore = "needs a live dev database (DATABASE_URL)"]
    async fn the_sweep_closes_every_reply_nobody_writes_and_only_those() {
        let pg = pg().await;
        let a = thread(&pg, "sweep-a").await;
        let b = thread(&pg, "sweep-b").await;
        let c = thread(&pg, "sweep-c").await;
        let orphan = reply(&pg, &a, 1, "left behind", 600.0).await;
        let (held, _writer) = held_reply(&pg, &b, "mine", 600.0).await;
        let fresh = reply(&pg, &c, 1, "elsewhere", 5.0).await;

        let closed = sweep_orphaned_replies(&pg).await.unwrap();
        // Closed by this sweep or by a sibling test's a moment earlier.
        assert!(
            closed.contains(&(orphan.clone(), a.clone())) || row(&pg, &orphan).await.0 == "error"
        );
        assert!(!closed.iter().any(|(m, _)| m == &held || m == &fresh));
        let (status, content, metadata) = row(&pg, &orphan).await;
        assert_eq!(status, "error");
        assert_eq!(content, format!("left behind\n\n{ORPHANED_LINE}"));
        assert_eq!(metadata["orphaned"], json!(true));
        assert_eq!(row(&pg, &held).await.0, "streaming");
        assert_eq!(row(&pg, &fresh).await.0, "streaming");
        for tag in ["sweep-a", "sweep-b", "sweep-c"] {
            cleanup(&pg, tag).await;
        }
    }

    /// What a send queued behind a reply reads, beat by beat.
    #[tokio::test]
    #[ignore = "needs a live dev database (DATABASE_URL)"]
    async fn a_queued_send_reads_where_its_reply_stands() {
        let pg = pg().await;
        let conv = thread(&pg, "fate").await;
        let id = reply(&pg, &conv, 1, "going", 5.0).await;
        assert_eq!(reply_fate(&pg, &id).await.unwrap(), ReplyFate::Writing);

        sqlx::query(
            "update messages set streamed_at = now() - interval '3 minutes' where id = $1::uuid",
        )
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
        assert_eq!(reply_fate(&pg, &id).await.unwrap(), ReplyFate::Orphaned);
        // Closed by that read, and still an orphan to the next one.
        assert_eq!(row(&pg, &id).await.0, "error");
        assert_eq!(reply_fate(&pg, &id).await.unwrap(), ReplyFate::Orphaned);

        let landed = reply(&pg, &conv, 2, "", 0.0).await;
        update_assistant(&pg, &landed, "done", "", &[], "complete")
            .await
            .unwrap();
        assert_eq!(reply_fate(&pg, &landed).await.unwrap(), ReplyFate::Landed);
        assert_eq!(
            reply_fate(&pg, "00000000-0000-4000-8000-00000000f8f8")
                .await
                .unwrap(),
            ReplyFate::Landed
        );
        cleanup(&pg, "fate").await;
    }

    /// Set when a reply was made and last stamped, merge `metadata` into it,
    /// and give it `tools`.
    async fn age(
        pg: &PgPool,
        id: String,
        created_secs_ago: f64,
        streamed_secs_ago: f64,
        metadata: Value,
        tools: Option<Value>,
    ) -> String {
        sqlx::query(
            "update messages set created_at = now() - make_interval(secs => $2), \
                streamed_at = now() - make_interval(secs => $3), \
                metadata = metadata || $4, tools = coalesce($5, tools) \
             where id = $1::uuid",
        )
        .bind(&id)
        .bind(created_secs_ago)
        .bind(streamed_secs_ago)
        .bind(metadata)
        .bind(tools)
        .execute(pg)
        .await
        .unwrap();
        id
    }

    /// A reply that has said nothing, and that no write has touched since it
    /// was made, may be a turn still waiting on the agent's response headers
    /// in a process that does not beat: the old container of the very deploy
    /// that ships the beat, which writes nothing to the row until the first
    /// chunk. Closed at 120 s, its thread read as idle and a second turn
    /// started beside the first. It has the longer lease. So does a resumed
    /// reply that has said nothing (the same wait, after a resurrect). One
    /// any write has touched since (a beat, a chunk) has the ordinary lease,
    /// and so does one that has begun.
    #[tokio::test]
    #[ignore = "needs a live dev database (DATABASE_URL)"]
    async fn a_reply_nothing_has_written_waits_out_the_headers_wait() {
        let pg = pg().await;
        let unwritten = thread(&pg, "unwritten").await;
        let resumed = thread(&pg, "resumed").await;
        let beaten = thread(&pg, "beaten").await;
        let tooled = thread(&pg, "tooled").await;
        // Made 200 s ago, and nothing written since: one clock reading.
        let waiting = age(
            &pg,
            reply(&pg, &unwritten, 1, "", 0.0).await,
            200.0,
            200.0,
            json!({}),
            None,
        )
        .await;
        // Made long ago, taken up again by a resume 200 s ago.
        let again = age(
            &pg,
            reply(&pg, &resumed, 1, "", 0.0).await,
            3600.0,
            200.0,
            json!({ "resumed": true }),
            None,
        )
        .await;
        // Made 210 s ago and beaten once since, then nothing.
        let left = age(
            &pg,
            reply(&pg, &beaten, 1, "", 0.0).await,
            210.0,
            200.0,
            json!({}),
            None,
        )
        .await;
        // Had begun: a tool call, last stamped 200 s ago.
        let working = age(
            &pg,
            reply(&pg, &tooled, 1, "", 0.0).await,
            210.0,
            200.0,
            json!({}),
            Some(json!([{ "name": "terminal", "label": "ls", "status": "running" }])),
        )
        .await;

        let closed = sweep_orphaned_replies(&pg).await.unwrap();
        for id in [&waiting, &again] {
            assert!(!closed.iter().any(|(m, _)| m == id));
            assert_eq!(row(&pg, id).await.0, "streaming");
            assert_eq!(reply_fate(&pg, id).await.unwrap(), ReplyFate::Writing);
        }
        assert_eq!(
            active_streaming_assistant(&pg, &unwritten).await.unwrap(),
            Some(waiting.clone())
        );
        assert_eq!(
            active_streaming_assistant(&pg, &resumed).await.unwrap(),
            Some(again.clone())
        );
        // Closed by this sweep or a sibling test's.
        for (conv, id) in [(&beaten, &left), (&tooled, &working)] {
            assert_eq!(active_streaming_assistant(&pg, conv).await.unwrap(), None);
            assert_eq!(row(&pg, id).await.0, "error");
        }

        // Past the hold and the headers wait, nothing is waiting any more.
        let waiting = age(&pg, waiting, 500.0, 500.0, json!({}), None).await;
        assert_eq!(
            active_streaming_assistant(&pg, &unwritten).await.unwrap(),
            None
        );
        assert_eq!(row(&pg, &waiting).await.0, "error");
        for tag in ["unwritten", "resumed", "beaten", "tooled"] {
            cleanup(&pg, tag).await;
        }
    }

    /// A reply closed as an orphan whose writer was alive after all (a
    /// process that did not beat, in a deploy's overlap) is written again by
    /// that writer: its next write is the writer's, so it is no longer an
    /// orphan, and the landed row must not say it was one.
    #[tokio::test]
    #[ignore = "needs a live dev database (DATABASE_URL)"]
    async fn a_reply_its_writer_comes_back_to_is_not_marked_orphaned() {
        let pg = pg().await;
        let conv = thread(&pg, "revived").await;
        let id = reply(&pg, &conv, 1, "half a", 200.0).await;
        assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
        assert_eq!(row(&pg, &id).await.2["orphaned"], json!(true));

        update_assistant(&pg, &id, "half a story", "", &[], "streaming")
            .await
            .unwrap();
        let (status, _, metadata) = row(&pg, &id).await;
        assert_eq!(status, "streaming");
        assert_eq!(metadata.get("orphaned"), None, "{metadata}");
        update_assistant(&pg, &id, "half a story, finished", "", &[], "complete")
            .await
            .unwrap();
        let (status, content, metadata) = row(&pg, &id).await;
        assert_eq!(
            (status.as_str(), content.as_str()),
            ("complete", "half a story, finished")
        );
        assert_eq!(metadata.get("orphaned"), None, "{metadata}");
        assert_eq!(reply_fate(&pg, &id).await.unwrap(), ReplyFate::Landed);
        cleanup(&pg, "revived").await;
    }
}
