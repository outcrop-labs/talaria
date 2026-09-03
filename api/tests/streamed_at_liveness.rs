// Live-DB proof of the long-turn liveness contract (cargo test -- --ignored).
// A turn is alive while it is WRITING (streamed_at), not while it is YOUNG
// (created_at) — the stale sweep, the working flags, and the one-shot
// auto-resume all key off that, and none of it is provable without Postgres
// evaluating the interval arithmetic the way it does. House rule: #[ignore]d,
// never CI.
//
//   DATABASE_URL=postgres://… cargo test --test streamed_at_liveness -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::conversations::{
    active_streaming_assistant, create_conversation, insert_streaming_assistant,
    mark_message_resumed, message_still_errored, prior_messages, resurrect_streaming_assistant,
    update_assistant,
};
use talaria_api::secretbox::SecretBox;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// One throwaway person per test, keyed by the test's own tag — the suite
/// runs its cases concurrently, so each cleans up ONLY its own user (a shared
/// pattern would delete a sibling test's person mid-flight and fail its
/// conversation insert on the users foreign key). Called before AND after;
/// the before-call sweeps a prior failed run's residue.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub = $1 or email = $2")
        .bind(format!("stream-live-{tag}"))
        .bind(format!("stream-live-{tag}@test.invalid"))
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, tag: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) \
         values ($1, $2, 'Stream Live', 'member') \
         returning id::text",
    )
    .bind(format!("stream-live-{tag}"))
    .bind(format!("stream-live-{tag}@test.invalid"))
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn land_user(pg: &PgPool, conversation: &str, seq: i32, content: &str) {
    sqlx::query(
        "insert into messages (conversation_id, seq, role, status, content) \
         values ($1::uuid, $2, 'user', 'complete', $3)",
    )
    .bind(conversation)
    .bind(seq)
    .bind(content)
    .execute(pg)
    .await
    .unwrap();
}

async fn status_of(pg: &PgPool, message: &str) -> String {
    let (status,): (String,) = sqlx::query_as("select status from messages where id = $1::uuid")
        .bind(message)
        .fetch_one(pg)
        .await
        .unwrap();
    status
}

/// A turn two HOURS old by created_at but still writing is mid-turn, not
/// stale. Under the old age doctrine this row was swept to error at ten
/// minutes — the "· interrupted" knowledgebase turns, killed by a clock that
/// measured the wrong thing.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_writing_turn_is_alive_however_old_its_row() {
    let pg = pool().await;
    cleanup(&pg, "alive").await;
    let user = person(&pg, "alive").await;
    let conv = create_conversation(&pg, &user, "claude-test", "Long turn", "chat", None)
        .await
        .unwrap();
    land_user(&pg, &conv, 0, "build the knowledgebase").await;
    let turn = insert_streaming_assistant(&pg, &conv, 1, &serde_json::json!({}))
        .await
        .unwrap();
    sqlx::query("update messages set created_at = now() - interval '2 hours' where id = $1::uuid")
        .bind(&turn)
        .execute(&pg)
        .await
        .unwrap();
    // The persist loop's throttled flush — the liveness signal itself.
    update_assistant(&pg, &turn, "still working", "", &[], "streaming")
        .await
        .unwrap();

    assert_eq!(
        active_streaming_assistant(&pg, &conv).await.unwrap(),
        Some(turn.clone())
    );
    assert_eq!(status_of(&pg, &turn).await, "streaming");
    cleanup(&pg, "alive").await;
}

/// A streaming row whose writer went silent past the sweep window dies with
/// an explanation in the row — never a bare error status the UI renders as
/// "interrupted" with nothing else. A row that died mid-prose keeps its prose.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_silent_writer_is_swept_to_an_explained_error() {
    let pg = pool().await;
    cleanup(&pg, "swept").await;
    let user = person(&pg, "swept").await;
    let conv = create_conversation(&pg, &user, "claude-test", "Swept turn", "chat", None)
        .await
        .unwrap();
    land_user(&pg, &conv, 0, "hello").await;
    let bare = insert_streaming_assistant(&pg, &conv, 1, &serde_json::json!({}))
        .await
        .unwrap();
    let midprose = insert_streaming_assistant(&pg, &conv, 2, &serde_json::json!({}))
        .await
        .unwrap();
    update_assistant(&pg, &midprose, "half a reply", "", &[], "streaming")
        .await
        .unwrap();
    // Both writers died: no flush for twenty minutes.
    for row in [&bare, &midprose] {
        sqlx::query(
            "update messages set streamed_at = now() - interval '20 minutes' where id = $1::uuid",
        )
        .bind(row)
        .execute(&pg)
        .await
        .unwrap();
    }

    // The sweep examines the NEWEST streaming row per call — one zombie per
    // look — so a conversation left with two converges: first call takes the
    // newer, second the one beneath it.
    assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
    assert_eq!(active_streaming_assistant(&pg, &conv).await.unwrap(), None);
    for (row, expected) in [(&bare, "(agent error:"), (&midprose, "half a reply")] {
        let (status, content): (String, String) =
            sqlx::query_as("select status, content from messages where id = $1::uuid")
                .bind(row)
                .fetch_one(&pg)
                .await
                .unwrap();
        assert_eq!(status, "error");
        assert!(
            content.starts_with(expected),
            "swept row keeps its own honest content: {content}"
        );
    }
    cleanup(&pg, "swept").await;
}

/// The auto-resume's DB contract, end to end: the gate opens for exactly one
/// attempt (message_still_errored flips when the resumed stamp lands), the
/// resurrection brings the SAME row back as the live turn, and the re-drive's
/// history excludes the dead attempt — the agent re-hears the ask, never its
/// own death notice.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_resume_gate_opens_once_and_the_re_drive_forgets_the_attempt() {
    let pg = pool().await;
    cleanup(&pg, "gate").await;
    let user = person(&pg, "gate").await;
    let conv = create_conversation(&pg, &user, "claude-test", "Resumed turn", "chat", None)
        .await
        .unwrap();
    land_user(&pg, &conv, 0, "move the doc").await;
    let turn = insert_streaming_assistant(&pg, &conv, 1, &serde_json::json!({}))
        .await
        .unwrap();
    let reason = "(agent error: the agent's stream went silent for 600s mid-turn)";
    update_assistant(&pg, &turn, reason, "", &[], "error")
        .await
        .unwrap();

    // The gate: open before the stamp, closed after — retry exactly once.
    assert!(message_still_errored(&pg, &turn).await.unwrap());
    mark_message_resumed(&pg, &turn).await.unwrap();
    assert!(!message_still_errored(&pg, &turn).await.unwrap());

    // The next turn's history sees the explained error; the re-drive doesn't.
    let sb = SecretBox::default();
    let history = prior_messages(&pg, &sb, &conv, None).await.unwrap();
    assert_eq!(history.len(), 2);
    let re_drive = prior_messages(&pg, &sb, &conv, Some(&turn)).await.unwrap();
    assert_eq!(re_drive.len(), 1);
    assert_eq!(re_drive[0].content, "move the doc");

    // The resurrection: same row, back to streaming, cleared — and it counts
    // as the conversation's live turn.
    resurrect_streaming_assistant(&pg, &turn).await.unwrap();
    assert_eq!(status_of(&pg, &turn).await, "streaming");
    let (content, tools): (String, serde_json::Value) =
        sqlx::query_as("select content, tools from messages where id = $1::uuid")
            .bind(&turn)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(content, "");
    assert_eq!(tools, serde_json::json!([]));
    assert_eq!(
        active_streaming_assistant(&pg, &conv).await.unwrap(),
        Some(turn)
    );
    cleanup(&pg, "gate").await;
}
