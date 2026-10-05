// Live-DB + live-Redis proof for agent-DM message ids, send times and
// reactions (comms polish U2): the detail read's new `id`/`createdAt`, the
// toggle route POST /api/conversations/{id}/messages/{msgId}/reactions, and
// the message_reactions cascade. The access rule and the "message belongs to
// this conversation" guard are WHERE clauses — only a real database proves
// them.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it conversation_reactions_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::json;
use talaria_api::conversations::{create_conversation, insert_user_message};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "conv-react-live";

async fn member(state: &AppState) -> (String, String, String) {
    let email = format!("{}@conv-react-live.invalid", uuid::Uuid::new_v4());
    let (id, cookie) = minted_member_with_email(state, PREFIX, &email).await;
    (id, cookie, email)
}

/// An agent DM with a user turn (seq 0) and an assistant reply (seq 1).
/// Returns (conversation id, user message id, assistant message id).
async fn dm(state: &AppState, owner: &str) -> (String, String, String) {
    let conv = create_conversation(&state.pg, owner, "assistant-live", "live dm", "chat", None)
        .await
        .unwrap();
    let user_msg = insert_user_message(
        &state.pg,
        &conv,
        0,
        "hi",
        &json!([]),
        Some(owner),
        &json!({}),
    )
    .await
    .unwrap();
    let (reply,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, content, status) \
         values ($1::uuid, 1, 'assistant', 'hello', 'complete') returning id::text",
    )
    .bind(&conv)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    (conv, user_msg, reply)
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn the_detail_carries_ids_and_send_times_in_seq_order() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (conv, user_msg, reply) = dm(&state, &me).await;
    let (status, body) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["id"], user_msg.as_str());
    assert_eq!(msgs[1]["id"], reply.as_str());
    for m in msgs {
        let at = m["createdAt"].as_str().expect("createdAt on every message");
        assert!(at.ends_with('Z') && at.contains('T'), "ISO time, got {at}");
        assert_eq!(m["reactions"], json!([]));
    }
}

/// AE2: 🙌 on an agent reply adds the chip; toggling again removes it.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_reaction_toggles_on_and_off() {
    let state = live_app_state().await;
    let (me, cookie, email) = member(&state).await;
    let (conv, _, reply) = dm(&state, &me).await;
    let path = format!("/api/conversations/{conv}/messages/{reply}/reactions");

    let (status, body) = call_json(
        &state,
        "POST",
        &path,
        Some(&cookie),
        Some(json!({ "emoji": "🙌" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "ok": true, "reacted": true }));
    let (_, detail) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(
        detail["messages"][1]["reactions"],
        json!([{ "emoji": "🙌", "actors": [email], "actorTypes": ["user"] }])
    );

    let (_, body) = call_json(
        &state,
        "POST",
        &path,
        Some(&cookie),
        Some(json!({ "emoji": "🙌" })),
    )
    .await;
    assert_eq!(body, json!({ "ok": true, "reacted": false }));
    let (_, detail) = call_json(
        &state,
        "GET",
        &format!("/api/conversations/{conv}"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(detail["messages"][1]["reactions"], json!([]));
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_message_from_another_conversation_is_a_404() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (conv_a, _, _) = dm(&state, &me).await;
    let (_, _, reply_b) = dm(&state, &me).await;
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv_a}/messages/{reply_b}/reactions"),
        Some(&cookie),
        Some(json!({ "emoji": "✅" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_stranger_gets_the_read_rules_404() {
    let state = live_app_state().await;
    let (owner, _, _) = member(&state).await;
    let (_, stranger_cookie, _) = member(&state).await;
    let (conv, _, reply) = dm(&state, &owner).await;
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/messages/{reply}/reactions"),
        Some(&stranger_cookie),
        Some(json!({ "emoji": "👀" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/messages/{reply}/reactions"),
        None,
        Some(json!({ "emoji": "👀" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn an_empty_or_overlong_emoji_is_a_400() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (conv, _, reply) = dm(&state, &me).await;
    let path = format!("/api/conversations/{conv}/messages/{reply}/reactions");
    for bad in [json!(""), json!("x".repeat(17))] {
        let (status, body) = call_json(
            &state,
            "POST",
            &path,
            Some(&cookie),
            Some(json!({ "emoji": bad })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn deleting_the_conversation_takes_its_reactions() {
    let state = live_app_state().await;
    let (me, cookie, _) = member(&state).await;
    let (conv, _, reply) = dm(&state, &me).await;
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/messages/{reply}/reactions"),
        Some(&cookie),
        Some(json!({ "emoji": "✅" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    sqlx::query("delete from conversations where id = $1::uuid")
        .bind(&conv)
        .execute(&state.pg)
        .await
        .unwrap();
    let left: i64 =
        sqlx::query_scalar("select count(*) from message_reactions where message_id = $1::uuid")
            .bind(&reply)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(left, 0);
}
