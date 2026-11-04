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
    sqlx::query("delete from channels where name = $1 or dm_key = $1")
        .bind(format!("working-live-{tag}"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where model like $1 || '-%'")
        .bind(format!("working-live-{tag}"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from research_runs where requested_by = $1")
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

#[tokio::test]
#[ignore]
async fn working_live_a_dm_is_named_after_the_other_person() {
    let (pg, user) = setup("dm").await;
    let peer = person(&pg, "dm-peer").await;
    let (room,): (String,) = sqlx::query_as(
        "insert into channels (name, kind, dm_key) values ('', 'dm', 'working-live-dm') \
         returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into channel_members (channel_id, user_id) \
         values ($1::uuid, $2::uuid), ($1::uuid, $3::uuid)",
    )
    .bind(&room)
    .bind(&user)
    .bind(&peer)
    .execute(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into channel_messages (channel_id, seq, author_type, author, status) \
         values ($1::uuid, 1, 'agent', 'working-live-agent', 'streaming')",
    )
    .bind(&room)
    .execute(&pg)
    .await
    .unwrap();

    let comms = read(&pg, &user)
        .await
        .unwrap()
        .comms
        .expect("the DM is working");
    assert_eq!(
        comms.label, "Working Live",
        "a DM shows the other person, never a blank name"
    );
    cleanup(&pg, "dm").await;
    cleanup(&pg, "dm-peer").await;
}

#[tokio::test]
#[ignore]
async fn working_live_an_untitled_chat_is_named_after_its_agent() {
    let (pg, user) = setup("untitled").await;
    let conv = create_conversation(&pg, &user, "working-live-agent", "", "chat", None)
        .await
        .unwrap();
    insert_streaming_assistant(&pg, &conv, 1, &json!({}))
        .await
        .unwrap();

    let comms = read(&pg, &user)
        .await
        .unwrap()
        .comms
        .expect("the chat is working");
    assert_eq!(
        comms.label, "working-live-agent",
        "an empty title falls back to the agent"
    );
    cleanup(&pg, "untitled").await;
}

#[tokio::test]
#[ignore]
async fn working_live_a_runs_streaming_thread_keeps_the_run_the_freshest() {
    let (pg, user) = setup("research").await;
    let older = create_conversation(&pg, &user, "working-live-agent", "older work", "work", None)
        .await
        .unwrap();
    let older_reply = insert_streaming_assistant(&pg, &older, 1, &json!({}))
        .await
        .unwrap();
    quiet_reply(&pg, &older_reply, 60.0).await;
    let thread = create_conversation(
        &pg,
        &user,
        "working-live-agent",
        "discussion",
        "research",
        None,
    )
    .await
    .unwrap();
    insert_streaming_assistant(&pg, &thread, 1, &json!({}))
        .await
        .unwrap();
    let (run,): (String,) = sqlx::query_as(
        "insert into research_runs \
           (owner_user_id, requested_by, agent_model, mode, question, title, status, \
            conversation_id, updated_at) \
         values ($1::uuid, $2, 'working-live-agent', 'recon', 'q', 'fresh research', 'running', \
                 $3::uuid, now() - interval '1 hour') \
         returning id::text",
    )
    .bind(&user)
    .bind("working-live-research")
    .bind(&thread)
    .fetch_one(&pg)
    .await
    .unwrap();

    assert_eq!(
        read(&pg, &user).await.unwrap().work,
        Some(Lead {
            count: 2,
            label: "fresh research".into(),
            href: format!("/research/{run}"),
        }),
        "the run and its streaming thread are one item, as fresh as the thread"
    );
    cleanup(&pg, "research").await;
}

#[tokio::test]
#[ignore]
async fn working_live_a_dm_of_only_agents_is_named_after_them() {
    let (pg, user) = setup("agents").await;
    sqlx::query(
        "insert into agent_defs (slug, department, model, display_name) values \
           ('working-live-agents-a', 'live', 'working-live-agents-a', 'Ada'), \
           ('working-live-agents-b', 'live', 'working-live-agents-b', 'Bo')",
    )
    .execute(&pg)
    .await
    .unwrap();
    let (room,): (String,) = sqlx::query_as(
        "insert into channels (name, kind, dm_key) values ('', 'dm', 'working-live-agents') \
         returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query("insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)")
        .bind(&room)
        .bind(&user)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query(
        "insert into channel_agents (channel_id, agent_model) \
         values ($1::uuid, 'working-live-agents-a'), ($1::uuid, 'working-live-agents-b')",
    )
    .bind(&room)
    .execute(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into channel_messages (channel_id, seq, author_type, author, status) \
         values ($1::uuid, 1, 'agent', 'working-live-agents-a', 'streaming')",
    )
    .bind(&room)
    .execute(&pg)
    .await
    .unwrap();

    let comms = read(&pg, &user)
        .await
        .unwrap()
        .comms
        .expect("the DM is working");
    assert_eq!(
        comms.label, "Ada, Bo",
        "a DM with no other person is named after its agents"
    );
    cleanup(&pg, "agents").await;
}
