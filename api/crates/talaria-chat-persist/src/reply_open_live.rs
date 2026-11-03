// Live-DB proof that a reply announces its start to the readers its end
// reaches, and to no one else. The audience is a query over Postgres, so these
// are `#[ignore]`d and CI's live-tests job runs them
// (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-chat-persist --lib -- --ignored reply_open_live

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_conversations::{add_plan_member, create_conversation};
use talaria_notify::NotifyDeps;

use super::{open_streaming_reply, reopen_streaming_reply};

type Rung = Arc<Mutex<Vec<(String, String)>>>;

async fn pool() -> PgPool {
    let url =
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("reply-open-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, tag: &str, who: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, 'Reply Open', 'member') \
         returning id::text",
    )
    .bind(format!("reply-open-{tag}-{who}"))
    .bind(format!("reply-open-{tag}-{who}@test.invalid"))
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// Notify deps whose publish edge records instead of sending.
fn recording(pg: &PgPool) -> (NotifyDeps, Rung) {
    let rung: Rung = Arc::new(Mutex::new(Vec::new()));
    let mut deps = NotifyDeps::publishing(pg.clone(), None);
    let sink = rung.clone();
    deps.realtime.publish = Arc::new(move |channel, payload| {
        sink.lock()
            .unwrap()
            .push((channel.to_string(), payload.to_string()));
    });
    (deps, rung)
}

/// The fan is detached, so give it a moment, then read who was rung.
async fn rung_for(rung: &Rung, wait: Duration) -> Vec<(String, String)> {
    tokio::time::sleep(wait).await;
    rung.lock().unwrap().clone()
}

#[tokio::test]
#[ignore]
async fn reply_open_live_rings_the_sender_of_a_chat() {
    let pg = pool().await;
    cleanup(&pg, "chat").await;
    let owner = person(&pg, "chat", "owner").await;
    let outsider = person(&pg, "chat", "outsider").await;
    let conv = create_conversation(&pg, &owner, "reply-open-agent", "chat", "chat", None)
        .await
        .unwrap();

    let (deps, rung) = recording(&pg);
    open_streaming_reply(&pg, deps, &conv, 1, &json!({}))
        .await
        .unwrap();
    let events = rung_for(&rung, Duration::from_millis(500)).await;

    let channels: BTreeSet<String> = events.iter().map(|(c, _)| c.clone()).collect();
    assert_eq!(
        channels,
        BTreeSet::from([format!("user:{owner}")]),
        "only the sender is rung"
    );
    assert!(!channels.contains(&format!("user:{outsider}")));
    let payload: Value = serde_json::from_str(&events[0].1).unwrap();
    assert_eq!(
        payload,
        json!({ "type": "conversation", "conversationId": conv }),
        "ids only"
    );
    cleanup(&pg, "chat").await;
}

#[tokio::test]
#[ignore]
async fn reply_open_live_rings_plan_members_and_no_one_else() {
    let pg = pool().await;
    cleanup(&pg, "plan").await;
    let owner = person(&pg, "plan", "owner").await;
    let member = person(&pg, "plan", "member").await;
    let outsider = person(&pg, "plan", "outsider").await;
    let conv = create_conversation(&pg, &owner, "reply-open-agent", "plan", "plan", None)
        .await
        .unwrap();
    add_plan_member(&pg, &conv, &member).await.unwrap();

    let (deps, rung) = recording(&pg);
    open_streaming_reply(&pg, deps, &conv, 1, &json!({}))
        .await
        .unwrap();
    let channels: BTreeSet<String> = rung_for(&rung, Duration::from_millis(500))
        .await
        .into_iter()
        .map(|(c, _)| c)
        .collect();

    assert_eq!(
        channels,
        BTreeSet::from([format!("user:{owner}"), format!("user:{member}")]),
    );
    assert!(!channels.contains(&format!("user:{outsider}")));
    cleanup(&pg, "plan").await;
}

/// A pool that refuses every query, for the write, beside a live one for the
/// audience: a fan that ran before the write would still find the owner.
async fn refusing_pool() -> PgPool {
    let pg = pool().await;
    pg.close().await;
    pg
}

#[tokio::test]
#[ignore]
async fn reply_open_live_rings_no_one_when_the_row_is_not_inserted() {
    let pg = pool().await;
    cleanup(&pg, "fail").await;
    let owner = person(&pg, "fail", "owner").await;
    let conv = create_conversation(&pg, &owner, "reply-open-agent", "chat", "chat", None)
        .await
        .unwrap();

    let (deps, rung) = recording(&pg);
    let refused = refusing_pool().await;
    assert!(
        open_streaming_reply(&refused, deps, &conv, 1, &json!({}))
            .await
            .is_err()
    );
    assert!(rung_for(&rung, Duration::from_millis(500)).await.is_empty());
    cleanup(&pg, "fail").await;
}

#[tokio::test]
#[ignore]
async fn reply_open_live_a_resumed_reply_rings_its_readers_again() {
    let pg = pool().await;
    cleanup(&pg, "resume").await;
    let owner = person(&pg, "resume", "owner").await;
    let member = person(&pg, "resume", "member").await;
    let conv = create_conversation(&pg, &owner, "reply-open-agent", "plan", "plan", None)
        .await
        .unwrap();
    add_plan_member(&pg, &conv, &member).await.unwrap();
    let (deps, _) = recording(&pg);
    let reply = open_streaming_reply(&pg, deps, &conv, 1, &json!({}))
        .await
        .unwrap();
    sqlx::query("update messages set status = 'error' where id = $1::uuid")
        .bind(&reply)
        .execute(&pg)
        .await
        .unwrap();

    let (deps, rung) = recording(&pg);
    let refused = refusing_pool().await;
    assert!(
        reopen_streaming_reply(&refused, deps, &conv, &reply)
            .await
            .is_err()
    );
    assert!(rung_for(&rung, Duration::from_millis(500)).await.is_empty());

    let (deps, rung) = recording(&pg);
    reopen_streaming_reply(&pg, deps, &conv, &reply)
        .await
        .unwrap();
    let channels: BTreeSet<String> = rung_for(&rung, Duration::from_millis(500))
        .await
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(
        channels,
        BTreeSet::from([format!("user:{owner}"), format!("user:{member}")]),
    );
    let (status,): (String,) = sqlx::query_as("select status from messages where id = $1::uuid")
        .bind(&reply)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(status, "streaming");
    cleanup(&pg, "resume").await;
}

/// The app's state with a real Redis, from REDIS_URL (CI's live-tests job sets
/// it), so the end event can be heard on the wire.
async fn wired_state() -> (talaria_state::AppState, String) {
    let url =
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the ignored live tests");
    let redis = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6399".into());
    let cfg = talaria_config::Config::from_parts(
        url.clone(),
        redis.clone(),
        "reply-open-live-root".into(),
        String::new(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    let pg = PgPool::connect(&url).await.expect("connect");
    (talaria_state::AppState::new(pg, Arc::new(cfg)), redis)
}

fn frames(frames: Vec<String>) -> talaria_gateway::fleet_chat::ByteStream {
    Box::pin(futures_util::stream::iter(
        frames.into_iter().map(|f| Ok(axum::body::Bytes::from(f))),
    ))
}

/// A reply that fails still rings its end, so the tab bar its start lit goes
/// dark at once instead of on its next poll.
async fn failed_reply_rings_its_end(tag: &str, stream: talaria_gateway::fleet_chat::ByteStream) {
    use futures_util::StreamExt;
    let (state, redis_url) = wired_state().await;
    let pg = state.pg.clone();
    cleanup(&pg, tag).await;
    let owner = person(&pg, tag, "owner").await;
    let conv = create_conversation(&pg, &owner, "reply-open-agent", "chat", "chat", None)
        .await
        .unwrap();
    let (deps, _) = recording(&pg);
    let reply = open_streaming_reply(&pg, deps, &conv, 1, &json!({}))
        .await
        .unwrap();

    let mut sub = redis::Client::open(redis_url.as_str())
        .unwrap()
        .get_async_pubsub()
        .await
        .expect("redis for the live test (REDIS_URL)");
    sub.subscribe(format!("user:{owner}")).await.unwrap();

    super::persist_assistant_stream(
        state.clone(),
        stream,
        reply.clone(),
        conv.clone(),
        None,
        None,
        super::OpeningTurn::take(&conv),
    )
    .await;

    let (status,): (String,) = sqlx::query_as("select status from messages where id = $1::uuid")
        .bind(&reply)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(status, "error");
    let mut messages = sub.on_message();
    let heard = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(m) = messages.next().await {
            let payload: String = m.get_payload().unwrap();
            if payload.contains(&conv) {
                return payload;
            }
        }
        String::new()
    })
    .await
    .expect("the failed reply's end was never heard");
    let payload: Value = serde_json::from_str(&heard).unwrap();
    assert_eq!(
        payload,
        json!({ "type": "conversation", "conversationId": conv })
    );
    cleanup(&pg, tag).await;
}

#[tokio::test]
#[ignore]
async fn reply_open_live_an_agent_error_rings_its_end() {
    let refused = r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"error"}],"error":{"message":"provider refused"}}"#;
    failed_reply_rings_its_end(
        "agent-error",
        frames(vec![format!("{refused}\n\n"), "data: [DONE]\n\n".into()]),
    )
    .await;
}

#[tokio::test]
#[ignore]
async fn reply_open_live_a_dead_stream_rings_its_end() {
    let role = r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"Once"},"finish_reason":null}]}"#;
    let reset = reqwest::get("http://127.0.0.1:1/")
        .await
        .expect_err("nothing listens on port 1");
    let items: Vec<reqwest::Result<axum::body::Bytes>> = vec![
        Ok(axum::body::Bytes::from(format!("{role}\n\n"))),
        Err(reset),
    ];
    failed_reply_rings_its_end("dead-stream", Box::pin(futures_util::stream::iter(items))).await;
}

/// A queued turn whose agent answers non-2xx opens its reply, then lands it
/// as an error: its readers hear both, so the glyph its start lit goes dark.
#[tokio::test]
#[ignore]
async fn reply_open_live_a_queued_turn_refused_by_its_agent_rings_its_end() {
    use futures_util::StreamExt;
    let (state, redis_url) = wired_state().await;
    let pg = state.pg.clone();
    cleanup(&pg, "refused").await;
    let owner = person(&pg, "refused", "owner").await;
    let conv = create_conversation(&pg, &owner, "reply-open-refuser", "chat", "chat", None)
        .await
        .unwrap();
    talaria_conversations::insert_user_message(
        &pg,
        &conv,
        1,
        "hi",
        &json!([]),
        Some(&owner),
        &json!({}),
    )
    .await
    .unwrap();

    let refuser = axum::Router::new()
        .fallback(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "refused") });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, refuser).await.unwrap() });
    let fleet = std::env::temp_dir().join(format!("reply-open-fleet-{}", std::process::id()));
    std::fs::create_dir_all(&fleet).unwrap();
    std::fs::write(
        fleet.join("fleet.json"),
        format!(r#"[{{"model":"reply-open-refuser","url":"{url}","key":null}}]"#),
    )
    .unwrap();
    // SAFETY: the only test in this binary that reads or writes this variable.
    unsafe { std::env::set_var("TALARIA_FLEET_DIR", &fleet) };

    let mut sub = redis::Client::open(redis_url.as_str())
        .unwrap()
        .get_async_pubsub()
        .await
        .expect("redis for the live test (REDIS_URL)");
    sub.subscribe(format!("user:{owner}")).await.unwrap();
    let meta = super::TurnMeta {
        agent_model: "reply-open-refuser".into(),
        tier: None,
        plan: None,
        research_prompt: None,
        work_prompts: vec![],
        sender: None,
    };
    super::continue_conversation(&state, &conv, &meta).await;

    let (status,): (String,) = sqlx::query_as(
        "select status from messages where conversation_id = $1::uuid and role = 'assistant'",
    )
    .bind(&conv)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(status, "error");
    let mut messages = sub.on_message();
    let mut heard = 0;
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(m) = messages.next().await {
            let payload: String = m.get_payload().unwrap();
            if payload.contains(&conv) {
                heard += 1;
            }
        }
    })
    .await;
    let _ = std::fs::remove_dir_all(&fleet);
    cleanup(&pg, "refused").await;
    assert_eq!(heard, 2, "the start and the end");
}
