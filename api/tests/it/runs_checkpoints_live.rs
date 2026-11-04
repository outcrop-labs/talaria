// Live-DB + live-Redis proof that a run's ledger keeps reply text out. A work
// session's checkpoint holds slices of the agent's reply, and only the
// transcript scrubs and prunes those, so neither the ledger nor the route that
// reads it may repeat one.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it runs_checkpoints_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::runs::store::{ClaimOutcome, NewRun, PgRunStore, RunStore};

use crate::support::{call_bytes, live_app_state, minted_member_with_email};

const PREFIX: &str = "runs-checkpoints-live";
const REPLY: &str = "reply slice that belongs to the transcript";

/// A member's runs and their ledgers go with the member.
async fn sweep(pg: &PgPool) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:%"))
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_ledger_and_its_route_hold_no_reply_text() {
    let state = live_app_state().await;
    sweep(&state.pg).await;
    let (owner, cookie) =
        minted_member_with_email(&state, PREFIX, "owner@runs-checkpoints-live.invalid").await;
    let store = PgRunStore::new(state.pg.clone());

    let id = uuid::Uuid::new_v4().to_string();
    store
        .insert(NewRun {
            id: id.clone(),
            kind: "rust-checkpoints-route-test".into(),
            owner_user_id: Some(owner),
            subject_type: None,
            subject_id: None,
            input: json!({}),
            phase: "queued".into(),
        })
        .await
        .unwrap();
    let token = format!("driver:{}", uuid::Uuid::new_v4());
    let claim = store.claim(&id, &token, 60_000).await.unwrap();
    assert!(matches!(claim, ClaimOutcome::Claimed { .. }));

    let reply = json!({"head": REPLY, "tail": REPLY, "checks": [REPLY]});
    let checkpoints = [
        (
            json!({"stage": "record", "turn": 1, "stageAttempt": 0, "said": "turn", "reply": reply}),
            "turn 1 answered",
        ),
        (
            json!({"stage": "failed", "turn": 2, "stageAttempt": 0, "error": REPLY}),
            "turn 2 failed",
        ),
        (
            json!({"stage": "send", "turn": 3, "stageAttempt": 0, "lastTail": REPLY}),
            "turn 2 recorded",
        ),
    ];
    for (checkpoint, phase) in checkpoints {
        store
            .checkpoint(&id, &token, checkpoint, phase.into(), false)
            .await
            .unwrap()
            .unwrap();
    }
    let run = store.get(&id).await.unwrap().unwrap();
    assert_eq!(run.checkpoint["lastTail"], REPLY, "the resume source keeps it");

    let path = format!("/api/runs/{id}/checkpoints");
    let (status, _, raw) = call_bytes(&state, "GET", &path, Some(&cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(raw).unwrap();
    assert!(!body.contains(REPLY), "the route repeated reply text: {body}");
    let parsed: Value = serde_json::from_str(&body).unwrap();
    let rows = parsed["checkpoints"].as_array().unwrap();
    let stages: Vec<_> = rows.iter().map(|r| (&r["status"], &r["turn"])).collect();
    assert_eq!(
        stages,
        [
            (&json!("record"), &json!(1)),
            (&json!("failed"), &json!(2)),
            (&json!("send"), &json!(3)),
        ],
        "stage and turn are what a rewind reads"
    );

    let stored: String = sqlx::query_scalar(
        "select jsonb_agg(to_jsonb(c))::text from run_checkpoints c where c.run_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert!(!stored.contains(REPLY), "the ledger stored reply text: {stored}");

    sweep(&state.pg).await;
}
