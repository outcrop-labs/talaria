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
