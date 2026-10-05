// Live-DB proof for POST /api/channels/{id}/typing: the gate (members only),
// the body (an optional boolean `typing`), and the answer. What it publishes
// is proven against the fake hub in realtime.rs; this proves the route in
// front of it against the real membership table, through the real router.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it typing_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::json;
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "typing-live";

async fn member(state: &AppState) -> (String, String) {
    let email = format!("{}@typing-live.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, PREFIX, &email).await
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

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn members_may_signal_typing_and_outsiders_may_not() {
    let state = live_app_state().await;
    let (me, cookie) = member(&state).await;
    let (them, _) = member(&state).await;
    let (outsider, outsider_cookie) = member(&state).await;
    let _ = outsider;
    let dm = room(&state, "dm", &[&me, &them]).await;
    let channel = room(&state, "channel", &[&me]).await;
    let url = |id: &str| format!("/api/channels/{id}/typing");

    // A DM member, typing and then not; an omitted flag means typing.
    for body in [
        json!({ "typing": true }),
        json!({ "typing": false }),
        json!({}),
    ] {
        let (status, out) = call_json(&state, "POST", &url(&dm), Some(&cookie), Some(body)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(out["ok"], true);
    }
    // A plain channel works too (it just has no firehose fan-out).
    let (status, _) = call_json(
        &state,
        "POST",
        &url(&channel),
        Some(&cookie),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Not a member: refused, and nothing is published for them.
    let (status, _) = call_json(
        &state,
        "POST",
        &url(&dm),
        Some(&outsider_cookie),
        Some(json!({ "typing": true })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Typing a thread reply names the thread; a malformed thread id is a 400.
    let root = uuid::Uuid::new_v4().to_string();
    let (status, _) = call_json(
        &state,
        "POST",
        &url(&channel),
        Some(&cookie),
        Some(json!({ "typing": true, "threadRootId": root })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call_json(
        &state,
        "POST",
        &url(&channel),
        Some(&cookie),
        Some(json!({ "threadRootId": "not-a-uuid" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A non-boolean flag is a 400, not a silent "typing".
    let (status, _) = call_json(
        &state,
        "POST",
        &url(&dm),
        Some(&cookie),
        Some(json!({ "typing": "yes" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
