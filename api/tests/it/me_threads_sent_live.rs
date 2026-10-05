// Live-DB + live-Redis proof for the Comms sidebar's Threads and Sent reads
// (comms polish U3): GET /api/me/threads and GET /api/me/sent. Both are one
// query whose correctness is a WHERE clause — channel membership, "the
// viewer's own message", roots with replies — so they are proven here
// against the real tables, through the real router.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it me_threads_sent_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::conversations::{create_conversation, insert_user_message};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "me-threads-live";

async fn member(state: &AppState) -> (String, String, String) {
    let email = format!("{}@me-threads-live.invalid", uuid::Uuid::new_v4());
    let (id, cookie) = minted_member_with_email(state, PREFIX, &email).await;
    (id, cookie, email)
}

async fn channel(state: &AppState, members: &[&str]) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into channels (name, kind) values ($1, 'channel') returning id::text",
    )
    .bind(format!("live-{}", uuid::Uuid::new_v4()))
    .fetch_one(&state.pg)
    .await
    .unwrap();
    for m in members {
        sqlx::query(
            "insert into channel_members (channel_id, user_id) values ($1::uuid, $2::uuid)",
        )
        .bind(&id)
        .bind(m)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    id
}

/// A user message by `author` (their email), optionally a reply to `root`.
async fn post(
    state: &AppState,
    channel: &str,
    author: &str,
    content: &str,
    root: Option<&str>,
) -> String {
    let (id,): (String,) = sqlx::query_as(
        "with s as (update channels set msg_seq = msg_seq + 1 where id = $1::uuid returning msg_seq) \
         insert into channel_messages (channel_id, seq, author_type, author, content, status, thread_root_id) \
         select $1::uuid, s.msg_seq, 'user', $2, $3, 'complete', $4::uuid from s returning id::text",
    )
    .bind(channel)
    .bind(author)
    .bind(content)
    .bind(root)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    id
}

fn root_ids(body: &Value) -> Vec<String> {
    body["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["root"]["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn threads_are_the_ones_i_started_or_replied_in() {
    let state = live_app_state().await;
    let (me, cookie, my_email) = member(&state).await;
    let (them, _, their_email) = member(&state).await;
    let ch = channel(&state, &[&me, &them]).await;

    // Theirs, and I replied: in.
    let replied = post(&state, &ch, &their_email, "their root", None).await;
    post(&state, &ch, &my_email, "my reply", Some(&replied)).await;
    // Mine, they replied: in.
    let started = post(&state, &ch, &my_email, "my root", None).await;
    post(&state, &ch, &their_email, "their reply", Some(&started)).await;
    // Mine, no replies: out (not a thread).
    let lonely = post(&state, &ch, &my_email, "nobody answered", None).await;
    // Theirs, only they replied: out (I was never in it).
    let not_mine = post(&state, &ch, &their_email, "side chat", None).await;
    post(&state, &ch, &their_email, "self reply", Some(&not_mine)).await;

    let (status, body) = call_json(&state, "GET", "/api/me/threads", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let ids = root_ids(&body);
    assert_eq!(
        ids,
        vec![started.clone(), replied.clone()],
        "newest reply first"
    );
    assert!(!ids.contains(&lonely));
    assert!(!ids.contains(&not_mine));
    let first = &body["threads"][0];
    assert_eq!(first["channelId"], ch.as_str());
    assert_eq!(first["channelKind"], "channel");
    assert_eq!(first["root"]["thread"]["count"], 1);

    // Leave the channel: its threads go with it.
    sqlx::query("delete from channel_members where channel_id = $1::uuid and user_id = $2::uuid")
        .bind(&ch)
        .bind(&me)
        .execute(&state.pg)
        .await
        .unwrap();
    let (_, body) = call_json(&state, "GET", "/api/me/threads", Some(&cookie), None).await;
    assert!(root_ids(&body).is_empty());
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn sent_merges_channel_and_agent_turns_and_only_mine() {
    let state = live_app_state().await;
    let (me, cookie, my_email) = member(&state).await;
    let (them, _, their_email) = member(&state).await;
    let ch = channel(&state, &[&me, &them]).await;

    let first = post(&state, &ch, &my_email, "first", None).await;
    post(&state, &ch, &their_email, "not mine", None).await;
    let conv = create_conversation(&state.pg, &me, "assistant-live", "ops chat", "chat", None)
        .await
        .unwrap();
    let turn = insert_user_message(
        &state.pg,
        &conv,
        0,
        "agent turn",
        &json!([]),
        Some(&me),
        &json!({}),
    )
    .await
    .unwrap();
    // Pin the order: the channel message an hour ago, the agent turn now.
    sqlx::query(
        "update channel_messages set created_at = now() - interval '1 hour' where id = $1::uuid",
    )
    .bind(&first)
    .execute(&state.pg)
    .await
    .unwrap();

    let (status, body) = call_json(&state, "GET", "/api/me/sent", Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let msgs = body["messages"].as_array().unwrap();
    let ids: Vec<&str> = msgs.iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec![turn.as_str(), first.as_str()]);
    assert_eq!(msgs[0]["kind"], "agent");
    assert_eq!(msgs[0]["conversationId"], conv.as_str());
    assert_eq!(msgs[0]["title"], "ops chat");
    assert_eq!(msgs[1]["kind"], "channel");
    assert_eq!(msgs[1]["conversationId"], ch.as_str());
    assert!(msgs.iter().all(|m| m["content"] != "not mine"));
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn both_reads_need_a_session() {
    let state = live_app_state().await;
    for path in ["/api/me/threads", "/api/me/sent"] {
        let (status, _) = call_json(&state, "GET", path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
}
