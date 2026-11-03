// Live-DB proof of how a stopped turn lands (release sweep chat-4). Stops
// reach Hermes on the completions arm, and a turn stopped before the model
// said anything ends with Hermes' own cancellation notice as its only
// content chunk ("Operation interrupted: waiting for model response (0.3s
// elapsed)."). The drain stored it as the agent's reply and filed "replied"
// for it. These drive the drain with the stream Hermes v0.22.4 writes
// (gateway/platforms/api_server_openai_routes.py: the role chunk, the
// `final_response` fallback when no delta went out, the `cancelled` finish)
// and read the row and the inbox it leaves. `#[ignore]`d — they need a
// database — and CI's live-tests job runs them
// (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-chat-persist --lib -- --ignored stop_landing_live

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_conversations::{create_conversation, insert_streaming_assistant, insert_user_message};
use talaria_gateway::fleet_chat::ByteStream;
use talaria_state::AppState;

use super::{OpeningTurn, persist_assistant_stream};

const CID: &str = "chatcmpl-5709a1e0b7c84d2f9e3a1b6c";

/// The app's state against the developer's Postgres. Redis is a port that
/// refuses at once: the drain's fan-out reads it best-effort, and the inbox
/// row these tests read is Postgres'.
async fn state() -> AppState {
    let url =
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the ignored live tests");
    let cfg = talaria_config::Config::from_parts(
        url.clone(),
        "redis://127.0.0.1:1".into(),
        "stop-landing-live-root".into(),
        String::new(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(PgPool::connect(&url).await.expect("connect"), Arc::new(cfg))
}

async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub = $1")
        .bind(format!("stop-landing-{tag}"))
        .execute(pg)
        .await
        .unwrap();
}

/// A person, their thread with one message in it, and the reply row a send
/// makes for it. Answers (person, thread, reply).
async fn turn(pg: &PgPool, tag: &str) -> (String, String, String) {
    cleanup(pg, tag).await;
    let (user,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, 'Stop Landing', 'member') \
         returning id::text",
    )
    .bind(format!("stop-landing-{tag}"))
    .bind(format!("stop-landing-{tag}@test.invalid"))
    .fetch_one(pg)
    .await
    .unwrap();
    let conv = create_conversation(
        pg,
        &user,
        "stop-landing-agent",
        "stop landing",
        "chat",
        None,
    )
    .await
    .unwrap();
    insert_user_message(
        pg,
        &conv,
        0,
        "tell me a story",
        &json!([]),
        None,
        &json!({}),
    )
    .await
    .unwrap();
    let reply = insert_streaming_assistant(pg, &conv, 1, &json!({}))
        .await
        .unwrap();
    (user, conv, reply)
}

fn chunk(delta: Value, finish: Option<&str>) -> String {
    let mut body = json!({
        "id": CID,
        "object": "chat.completion.chunk",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
    });
    if finish == Some("cancelled") {
        body["hermes"] = json!({
            "completed": false, "partial": false, "failed": false,
            "error": null, "error_code": "agent_error",
        });
    }
    format!("data: {body}\n\n")
}

/// The stream Hermes writes for a turn: the role chunk, `said` as content
/// (the deltas, or the fallback chunk), and the finish.
fn hermes(said: &[&str], finish: &str) -> ByteStream {
    let mut frames = vec![chunk(json!({ "role": "assistant" }), None)];
    frames.extend(said.iter().map(|s| chunk(json!({ "content": s }), None)));
    frames.push(chunk(json!({}), Some(finish)));
    frames.push("data: [DONE]\n\n".into());
    Box::pin(futures_util::stream::iter(
        frames.into_iter().map(|f| Ok(Bytes::from(f))),
    ))
}

async fn drain(state: &AppState, conv: &str, reply: &str, body: ByteStream) {
    persist_assistant_stream(
        state.clone(),
        body,
        reply.to_string(),
        conv.to_string(),
        None,
        None,
        OpeningTurn::take(conv),
    )
    .await;
}

async fn landed(pg: &PgPool, reply: &str) -> (String, String, Value) {
    sqlx::query_as("select status, content, metadata from messages where id = $1::uuid")
        .bind(reply)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// The person's agent-reply rows, read for up to `within`: the reply
/// notification is filed on a task of its own after the landing.
async fn rings(pg: &PgPool, user: &str, within: Duration) -> Vec<String> {
    let until = tokio::time::Instant::now() + within;
    loop {
        let bodies: Vec<(String,)> = sqlx::query_as(
            "select body from notifications where user_id = $1::uuid and kind = 'agent-reply'",
        )
        .bind(user)
        .fetch_all(pg)
        .await
        .unwrap();
        if !bodies.is_empty() || tokio::time::Instant::now() >= until {
            return bodies.into_iter().map(|(b,)| b).collect();
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The control: a reply that finished rings its away reader, so the empty
/// inboxes below are the drain's choice and not a probe that cannot see.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_finished_reply_rings() {
    let state = state().await;
    let (user, conv, reply) = turn(&state.pg, "finished").await;
    drain(
        &state,
        &conv,
        &reply,
        hermes(&["Once upon a time."], "stop"),
    )
    .await;

    let (status, content, _) = landed(&state.pg, &reply).await;
    assert_eq!(
        (status.as_str(), content.as_str()),
        ("complete", "Once upon a time.")
    );
    assert_eq!(
        rings(&state.pg, &user, Duration::from_secs(10)).await,
        ["Once upon a time."]
    );
    cleanup(&state.pg, "finished").await;
}

/// Every notice Hermes v0.22.4 ends a turn with when it was stopped before
/// the model said anything. Each lands as an empty stopped turn (the page
/// draws "stopped"), so it is neither the agent's reply on reload nor sent
/// back to the model as its earlier answer, and nobody is told it "replied".
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_stop_before_the_model_spoke_lands_empty_and_rings_nobody() {
    let state = state().await;
    for (n, notice) in [
        "Operation interrupted: waiting for model response (0.3s elapsed).",
        "Operation interrupted: waiting for model response (1.2s elapsed).",
        "Operation interrupted: retrying API call after error (retry 1/3).",
        "Operation interrupted: waiting for the provider to recover (cycle 1/3).",
        "Operation interrupted.",
        "Stopped before this turn started. Your message was not processed.",
        "Stopped waiting for another Hermes process on this session. Your message was not processed.",
    ]
    .into_iter()
    .enumerate()
    {
        let tag = format!("before-{n}");
        let (user, conv, reply) = turn(&state.pg, &tag).await;
        drain(&state, &conv, &reply, hermes(&[notice], "cancelled")).await;

        let (status, content, metadata) = landed(&state.pg, &reply).await;
        assert_eq!(status, "complete", "{notice}");
        assert_eq!(content, "", "{notice}");
        assert_eq!(metadata["stopped"], json!(true), "{notice}");
        assert!(rings(&state.pg, &user, Duration::from_secs(3)).await.is_empty(), "{notice}");
        cleanup(&state.pg, &tag).await;
    }
}

/// A stop after the agent had begun keeps its words, and rings nobody: the
/// person who pressed Stop is the one it would have told.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_stop_after_words_keeps_them_and_rings_nobody() {
    let state = state().await;
    let (user, conv, reply) = turn(&state.pg, "after").await;
    drain(
        &state,
        &conv,
        &reply,
        hermes(&["The lamp at ", "Garrick Point"], "cancelled"),
    )
    .await;

    let (status, content, metadata) = landed(&state.pg, &reply).await;
    assert_eq!(
        (status.as_str(), content.as_str()),
        ("complete", "The lamp at Garrick Point")
    );
    assert_eq!(metadata["stopped"], json!(true));
    assert!(
        rings(&state.pg, &user, Duration::from_secs(3))
            .await
            .is_empty()
    );
    cleanup(&state.pg, "after").await;
}
