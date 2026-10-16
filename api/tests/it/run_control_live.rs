// Live-DB proof for the two run-control routes — POST /api/conversations/{id}
// /stop and /steer — on the half that needs no Hermes: the "nothing is
// streaming" refusals, and what each does with a turn that is streaming
// through the full-history arm (no run id). The Hermes half — a run actually
// interrupted, a steer actually taken mid-turn — is the live script in the
// changelog, against a hired container.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it run_control_live:: -- --ignored
use axum::http::StatusCode;
use serde_json::json;
use talaria_api::conversations::{create_conversation, insert_user_message};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "run-control-live";

async fn member(state: &AppState) -> (String, String) {
    let email = format!("{}@run-control-live.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, PREFIX, &email).await
}

/// A conversation with one user turn and, when asked, a STREAMING assistant
/// row with no run id — the full-history arm's shape mid-turn.
async fn dm(state: &AppState, owner: &str, streaming: bool) -> String {
    let conv = create_conversation(&state.pg, owner, "assistant-live", "live dm", "chat", None)
        .await
        .unwrap();
    insert_user_message(
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
    if streaming {
        sqlx::query(
            "insert into messages (conversation_id, seq, role, content, status) \
             values ($1::uuid, 1, 'assistant', '', 'streaming')",
        )
        .bind(&conv)
        .execute(&state.pg)
        .await
        .unwrap();
    }
    conv
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn with_nothing_streaming_both_routes_refuse_with_a_409() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state).await;
    let conv = dm(&state, &me, false).await;
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/stop"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/steer"),
        Some(&cookie),
        Some(json!({ "content": "turn left" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // ...and the refused steer wrote nothing: the person meant to send a
    // message, and gets to.
    let (n,): (i64,) =
        sqlx::query_as("select count(*) from messages where conversation_id = $1::uuid")
            .bind(&conv)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_full_history_turn_is_stopped_by_the_client_and_steered_by_the_queue() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state).await;
    let conv = dm(&state, &me, true).await;
    // No run id: the client's own abort is the stop, and the route says so.
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/stop"),
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["hermes"], false);
    // The steer lands as a queued row, stamped — and never as a second turn.
    let (status, body) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/steer"),
        Some(&cookie),
        Some(json!({ "content": "turn left" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["queued"], true);
    let row = body["messageId"]
        .as_str()
        .expect("the row's id")
        .to_string();
    let (role, content, metadata): (String, String, serde_json::Value) =
        sqlx::query_as("select role, content, metadata from messages where id = $1::uuid")
            .bind(&row)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    assert_eq!(role, "user");
    assert_eq!(content, "turn left");
    assert_eq!(metadata["steer"], true);
    assert!(metadata.get("steer_accepted").is_none(), "{metadata}");
    // A stranger cannot steer or stop somebody else's thread — same 403 for
    // both, as the conversation read answers.
    let (_, stranger) = member(&state).await;
    let (status, _) = call_json(
        &state,
        "POST",
        &format!("/api/conversations/{conv}/steer"),
        Some(&stranger),
        Some(json!({ "content": "mine now" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
