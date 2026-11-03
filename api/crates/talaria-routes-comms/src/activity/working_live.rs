// Live-DB proof of /api/working's comms arm: a streaming reply lights it, and
// the same reply gone silent past its window does not. The windows are WHERE
// clauses only Postgres can confirm, so these are `#[ignore]`d and CI's
// live-tests job runs them (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-routes-comms --lib -- --ignored working_live

use serde_json::json;
use sqlx::PgPool;
use talaria_conversations::{create_conversation, insert_streaming_assistant};

use super::*;

/// One throwaway person per tag, swept before and after. The cascade takes
/// their conversations and messages; rooms do not cascade from a person, so
/// they go by name.
async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from channels where name = $1")
        .bind(format!("working-live-{tag}"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub = $1")
        .bind(format!("working-live-{tag}"))
        .execute(pg)
        .await
        .unwrap();
}

async fn setup(tag: &str) -> (PgPool, String) {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
    let pg = PgPool::connect(&url).await.expect("connect");
    let user = person(&pg, tag).await;
    (pg, user)
}

async fn person(pg: &PgPool, tag: &str) -> String {
    cleanup(pg, tag).await;
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, 'Working Live', 'member') \
         returning id::text",
    )
    .bind(format!("working-live-{tag}"))
    .bind(format!("working-live-{tag}@test.invalid"))
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn quiet_reply(pg: &PgPool, message_id: &str, quiet_secs: f64) {
    sqlx::query(
        "update messages set streamed_at = now() - make_interval(secs => $2) where id = $1::uuid",
    )
    .bind(message_id)
    .bind(quiet_secs)
    .execute(pg)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore]
async fn working_live_chat_reply_lights_comms_until_it_goes_silent() {
    let (pg, user) = setup("chat").await;
    let conv = create_conversation(
        &pg,
        &user,
        "working-live-agent",
        "working live chat",
        "chat",
        None,
    )
    .await
    .unwrap();
    let reply = insert_streaming_assistant(&pg, &conv, 1, &json!({}))
        .await
        .unwrap();

    let working = read(&pg, &user).await.unwrap();
    assert_eq!(
        working.comms,
        Some(Lead {
            count: 1,
            label: "working live chat".into(),
            href: format!("/comms/agent/working-live-agent/{conv}"),
        }),
        "a streaming chat reply counts"
    );
    assert_eq!(working.work, None, "a chat is not Work");

    quiet_reply(&pg, &reply, 6.0 * 60.0).await;
    assert_eq!(
        read(&pg, &user).await.unwrap().comms,
        None,
        "a reply silent past the five-minute window does not count"
    );
    cleanup(&pg, "chat").await;
}

#[tokio::test]
#[ignore]
async fn working_live_room_stream_lights_comms_until_it_goes_silent() {
    let (pg, user) = setup("room").await;
    let (room,): (String,) =
        sqlx::query_as("insert into channels (name) values ($1) returning id::text")
            .bind("working-live-room")
            .fetch_one(&pg)
            .await
            .unwrap();
    sqlx::query("insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&room)
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
    let (message,): (String,) = sqlx::query_as(
        "insert into channel_messages (channel_id, seq, author_type, author, status) \
         values ($1::uuid, 1, 'agent', 'working-live-agent', 'streaming') returning id::text",
    )
    .bind(&room)
    .fetch_one(&pg)
    .await
    .unwrap();

    assert_eq!(
        read(&pg, &user).await.unwrap().comms,
        Some(Lead {
            count: 1,
            label: "working-live-room".into(),
            href: format!("/comms/channel/{room}"),
        }),
        "a streaming agent reply in a member's room counts"
    );

    sqlx::query(
        "update channel_messages \
         set created_at = now() - interval '16 minutes', edited_at = now() - interval '16 minutes' \
         where id = $1::uuid",
    )
    .bind(&message)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(
        read(&pg, &user).await.unwrap().comms,
        None,
        "a room reply silent past the fifteen-minute window does not count"
    );
    cleanup(&pg, "room").await;
}

#[tokio::test]
#[ignore]
async fn working_live_someone_elses_chat_never_counts() {
    let (pg, owner) = setup("owner").await;
    let other = person(&pg, "other").await;
    let conv = create_conversation(&pg, &owner, "working-live-agent", "not yours", "chat", None)
        .await
        .unwrap();
    insert_streaming_assistant(&pg, &conv, 1, &json!({}))
        .await
        .unwrap();

    assert_eq!(
        read(&pg, &other).await.unwrap().comms,
        None,
        "a chat admits only its owner"
    );
    cleanup(&pg, "owner").await;
    cleanup(&pg, "other").await;
}
