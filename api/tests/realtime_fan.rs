// Live-DB proof of the two rail fans (cargo test -- --ignored): who a
// channel event reaches (every member, nobody else) and who a conversation
// event reaches (the owner, plus a plan's members). The publish edge is the
// same fake the realtime suite uses — captured topics, no Redis — because
// the question is ROUTING, not delivery. House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test realtime_fan -- --ignored

use futures_util::FutureExt;
use std::sync::{Arc, Mutex};
use talaria_api::notify::{NotifyDeps, fan_channel_event, fan_conversation_event};
use talaria_api::realtime::RealtimeDeps;

async fn pool() -> sqlx::postgres::PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    sqlx::postgres::PgPool::connect(&url)
        .await
        .expect("connect")
}

/// Delete the throwaway people; the cascade takes their channels and plans.
async fn cleanup(pg: &sqlx::PgPool) {
    sqlx::query("delete from users where email like 'fan-test-%@test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &sqlx::PgPool, sub: &str, email: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') returning id::text",
    )
    .bind(sub)
    .bind(email)
    .bind(sub)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// The captured publish edge: every (topic, payload) the fans send, in order.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<String>>>);

impl Captured {
    fn deps(&self) -> RealtimeDeps {
        let seen = self.0.clone();
        RealtimeDeps {
            publish: Arc::new(move |topic: &str, payload: &str| {
                seen.lock().unwrap().push(format!("{topic} {payload}"));
            }),
            subscribe: Arc::new(|_| async { tokio::sync::mpsc::channel::<String>(1).1 }.boxed()),
        }
    }

    fn topics(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.split(' ').next().unwrap().to_string())
            .collect()
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

/// The fans spawn their work; wait until the expected number of publishes
/// have landed (or fail after a generous two seconds — a real fan takes one
/// query).
async fn settle(seen: &Captured, expected: usize) {
    for _ in 0..200 {
        if seen.len() >= expected {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "the fan did not settle on {expected} publishes in 2s — saw {:?}",
        seen.topics()
    );
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_fans_reach_exactly_their_audiences() {
    let pg = pool().await;
    cleanup(&pg).await;
    let owner = person(&pg, "fan-test-owner", "fan-test-owner@test.invalid").await;
    let mate = person(&pg, "fan-test-mate", "fan-test-mate@test.invalid").await;
    let stranger = person(&pg, "fan-test-out", "fan-test-out@test.invalid").await;

    // — A channel with two members: the event reaches each member's own
    // firehose, and the payload is the id-shaped event, not the message.
    let (channel,): (String,) = sqlx::query_as(
        "insert into channels (name, created_by) values ('fan-test-room', $1::uuid) returning id::text",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    for member in [&owner, &mate] {
        sqlx::query(
            "insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)",
        )
        .bind(&channel)
        .bind(member)
        .execute(&pg)
        .await
        .unwrap();
    }
    let seen = Captured::default();
    fan_channel_event(
        NotifyDeps {
            pg: pg.clone(),
            realtime: seen.deps(),
        },
        channel.clone(),
    );
    settle(&seen, 2).await;
    let mut topics = seen.topics();
    topics.sort();
    assert_eq!(
        topics,
        [format!("user:{owner}"), format!("user:{mate}")],
        "every member, and nobody else — the stranger never appears"
    );
    assert_eq!(
        seen.0.lock().unwrap()[0].split_once(' ').unwrap().1,
        format!(r#"{{"type":"channel","channelId":"{channel}"}}"#)
    );

    // — A plan owned by the mate, shared with the owner: the conversation
    // event reaches both. A chat thread would reach its owner alone.
    let (plan,): (String,) = sqlx::query_as(
        "insert into conversations (user_id, agent_model, kind) values ($1::uuid, 'claude-test', 'plan') returning id::text",
    )
    .bind(&mate)
    .fetch_one(&pg)
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
    let seen = Captured::default();
    fan_conversation_event(
        NotifyDeps {
            pg: pg.clone(),
            realtime: seen.deps(),
        },
        plan.clone(),
    );
    settle(&seen, 2).await;
    let mut topics = seen.topics();
    topics.sort();
    assert_eq!(topics, [format!("user:{owner}"), format!("user:{mate}")]);
    assert_eq!(
        seen.0.lock().unwrap()[0].split_once(' ').unwrap().1,
        format!(r#"{{"type":"conversation","conversationId":"{plan}"}}"#)
    );
    assert!(
        !topics.contains(&format!("user:{stranger}")),
        "a person who cannot read the thread never hears about it"
    );

    cleanup(&pg).await;
}
