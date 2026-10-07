// Live-DB + live-Redis proof for the Comms profile drawer: the job title
// (PUT /api/me { title }, read back on GET /api/me and GET /api/users beside
// the person's time zone) and the two "conversations with" reads —
// GET /api/users/{id}/conversations and GET /api/agents/{id}/conversations.
// Both reads are one WHERE clause over membership (both parties, never
// archived, never a task room) and an agent's seat or DM threads, so they are
// proven here against the real tables, through the real router.
//
// The pure halves (the patch bounds, the merge order and wire, the untitled
// thread label) are unit-tested beside their code.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it profile_drawer_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use talaria_api::conversations::create_conversation;
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "profile-drawer-live";

async fn member(state: &AppState) -> (String, String, String) {
    let email = format!("{}@profile-drawer-live.invalid", uuid::Uuid::new_v4());
    let (id, cookie) = minted_member_with_email(state, PREFIX, &email).await;
    (id, cookie, email)
}

async fn room(state: &AppState, kind: &str, members: &[&str]) -> String {
    let (id,): (String,) =
        sqlx::query_as("insert into channels (name, kind) values ($1, $2) returning id::text")
            .bind(format!("live-{}", uuid::Uuid::new_v4()))
            .bind(kind)
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

async fn post(state: &AppState, channel: &str, author: &str, ago_minutes: i32) {
    sqlx::query(
        "with s as (update channels set msg_seq = msg_seq + 1 where id = $1::uuid returning msg_seq) \
         insert into channel_messages (channel_id, seq, author_type, author, content, status, created_at) \
         select $1::uuid, s.msg_seq, 'user', $2, 'hi', 'complete', \
           now() - make_interval(mins => $3) from s",
    )
    .bind(channel)
    .bind(author)
    .bind(ago_minutes)
    .execute(&state.pg)
    .await
    .unwrap();
}

fn ids(body: &Value) -> Vec<String> {
    body["conversations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn title_is_set_trimmed_cleared_and_listed_with_timezone() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;

    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "title": "  Product designer ", "timezone": "Asia/Kolkata" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, prefs) = call_json(&state, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(prefs["title"], "Product designer");

    let (_, dir) = call_json(&state, "GET", "/api/users", Some(&cookie), None).await;
    let row = dir["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == me.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["title"], "Product designer");
    assert_eq!(row["timezone"], "Asia/Kolkata");

    // Blank clears, like status text; too long is a 400 that writes nothing.
    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "title": "x".repeat(81) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call_json(
        &state,
        "PUT",
        "/api/me",
        Some(&cookie),
        Some(json!({ "title": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, prefs) = call_json(&state, "GET", "/api/me", Some(&cookie), None).await;
    assert_eq!(prefs["title"], Value::Null);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn person_conversations_are_the_rooms_we_both_are_in() {
    let state = live_app_state().await;
    let (me, cookie, my_email) = member(&state).await;
    let (them, _, their_email) = member(&state).await;
    let (other, _, _) = member(&state).await;

    let dm = room(&state, "dm", &[&me, &them]).await;
    let chan = room(&state, "channel", &[&me, &them, &other]).await;
    let relay = room(&state, "group", &[&me, &them]).await;
    // Theirs without me, mine without them: out.
    let theirs = room(&state, "channel", &[&them, &other]).await;
    let mine = room(&state, "channel", &[&me, &other]).await;
    // An archived room: out, even with both of us in it.
    let archived = room(&state, "channel", &[&me, &them]).await;
    sqlx::query("update channels set archived_at = now() where id = $1::uuid")
        .bind(&archived)
        .execute(&state.pg)
        .await
        .unwrap();

    // Order by last message: relay newest, then dm, then the channel.
    post(&state, &chan, &their_email, 30).await;
    post(&state, &dm, &their_email, 10).await;
    post(&state, &dm, &my_email, 9).await;
    post(&state, &relay, &their_email, 1).await;
    sqlx::query(
        "update channels set updated_at = now() - interval '1 day' where id = any($1::uuid[])",
    )
    .bind([&dm, &chan, &relay])
    .execute(&state.pg)
    .await
    .unwrap();

    let path = format!("/api/users/{them}/conversations");
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), vec![relay.clone(), dm.clone(), chan.clone()]);
    for gone in [&theirs, &mine, &archived] {
        assert!(!ids(&body).contains(gone));
    }
    let dm_row = &body["conversations"][1];
    assert_eq!(dm_row["kind"], "dm");
    assert_eq!(
        dm_row["name"], their_email,
        "a DM is named for the other person"
    );
    // Their one message is unread; my own reply never counts.
    assert_eq!(dm_row["unreadCount"], 1);
    assert_eq!(body["conversations"][0]["kind"], "group");

    // Unknown and malformed ids are 404s; no session is a 401.
    let unknown = format!("/api/users/{}/conversations", uuid::Uuid::new_v4());
    let (status, _) = call_json(&state, "GET", &unknown, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &state,
        "GET",
        "/api/users/not-a-uuid/conversations",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(&state, "GET", &path, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn agent_conversations_merge_rooms_and_my_threads() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (them, _, _) = member(&state).await;
    let model = format!("drawer-live-{}", uuid::Uuid::new_v4().simple());

    let seated = room(&state, "group", &[&me]).await;
    let not_mine = room(&state, "channel", &[&them]).await;
    let no_agent = room(&state, "channel", &[&me]).await;
    for ch in [&seated, &not_mine] {
        sqlx::query("insert into channel_agents (channel_id, agent_model) values ($1::uuid, $2)")
            .bind(ch)
            .bind(&model)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    sqlx::query("update channels set updated_at = now() - interval '2 hours' where id = $1::uuid")
        .bind(&seated)
        .execute(&state.pg)
        .await
        .unwrap();
    let thread = create_conversation(&state.pg, &me, &model, "", "chat", None)
        .await
        .unwrap();
    // Someone else's thread with the same agent: never mine to list.
    let theirs = create_conversation(&state.pg, &them, &model, "theirs", "chat", None)
        .await
        .unwrap();

    let path = format!("/api/agents/{model}/conversations");
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(ids(&body), vec![thread.clone(), seated.clone()]);
    for gone in [&not_mine, &no_agent, &theirs] {
        assert!(!ids(&body).contains(gone));
    }
    assert_eq!(body["conversations"][0]["kind"], "agent");
    assert_eq!(body["conversations"][0]["name"], "New thread");
    assert_eq!(body["conversations"][1]["kind"], "group");

    // Outside my access list → 404, the same answer as no such agent.
    sqlx::query(
        "insert into user_agent_access (user_id, agent_model) values ($1::uuid, 'someone-else')",
    )
    .bind(&me)
    .execute(&state.pg)
    .await
    .unwrap();
    let (status, _) = call_json(&state, "GET", &path, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
