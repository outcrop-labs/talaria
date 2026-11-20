// Live-DB proof that a work session with no checkpoint yet (`runs.checkpoint`
// NULL: queued, or still in its first turn) reads as a session with no turn,
// through the real router, on both live-session reads: the ticket's
// GET /api/tasks/{id}/work-session and the board's
// GET /api/boards/{id}/work-sessions. A NULL into a plain `serde_json::Value`
// is a decode panic, which the router's catch-panic layer answers as a 500,
// and on the board read one queued card took every card's live state with it.
// Only a real row asks that decode question.
//
// House rule: #[ignore]d, run by CI's live job.
//
//   source ui/.env && cargo test -p talaria-api --test it work_session_null_checkpoint_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email, sweep_user_rows};

const PREFIX: &str = "work-session-null-checkpoint-live";

/// A board `owner` owns and is the only member of, with one ticket per title.
async fn board_with_tickets(pg: &PgPool, owner: &str, titles: &[&str]) -> (String, Vec<String>) {
    let board: String = sqlx::query_scalar(
        "insert into boards (name, owner_id) values ('null checkpoint live', $1::uuid) \
         returning id::text",
    )
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board)
    .bind(owner)
    .execute(pg)
    .await
    .unwrap();
    let mut tasks = Vec::new();
    for title in titles {
        let id: String = sqlx::query_scalar(
            "insert into tasks (board_id, title) values ($1::uuid, $2) returning id::text",
        )
        .bind(&board)
        .bind(title)
        .fetch_one(pg)
        .await
        .unwrap();
        tasks.push(id);
    }
    (board, tasks)
}

async fn session(
    pg: &PgPool,
    tag: &str,
    task_id: &str,
    state: &str,
    checkpoint: Option<Value>,
) -> String {
    sqlx::query_scalar(
        "insert into runs (kind, subject_type, subject_id, state, input, checkpoint) \
         values ('work-session', 'task', $1, $2, $3, $4) returning id::text",
    )
    .bind(task_id)
    .bind(state)
    .bind(json!({ "agentModel": model(tag), "taskId": task_id }))
    .bind(checkpoint)
    .fetch_one(pg)
    .await
    .unwrap()
}

fn model(tag: &str) -> String {
    format!("{PREFIX}-{tag}")
}

async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from runs where input->>'agentModel' = $1")
        .bind(model(tag))
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, PREFIX, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_queued_session_with_no_checkpoint_reads_on_the_ticket() {
    let state: AppState = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "ticket").await;
    let email = format!("owner-ticket@{PREFIX}.invalid");
    let (owner, cookie) =
        minted_member_with_email(&state, &format!("{PREFIX}:ticket"), &email).await;
    let (_, tasks) = board_with_tickets(&pg, &owner, &["queued"]).await;
    let queued = session(&pg, "ticket", &tasks[0], "queued", None).await;

    let path = format!("/api/tasks/{}/work-session", tasks[0]);
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["session"]["runId"], queued, "{body}");
    assert_eq!(body["session"]["state"], "queued", "{body}");
    assert_eq!(body["session"]["turn"], Value::Null, "{body}");
    assert_eq!(body["session"]["lastTail"], Value::Null, "{body}");
    sweep(&pg, "ticket").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn one_queued_card_leaves_the_board_s_other_sessions_readable() {
    let state: AppState = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "board").await;
    let email = format!("owner-board@{PREFIX}.invalid");
    let (owner, cookie) =
        minted_member_with_email(&state, &format!("{PREFIX}:board"), &email).await;
    let (board, tasks) = board_with_tickets(&pg, &owner, &["queued", "working"]).await;
    let queued = session(&pg, "board", &tasks[0], "queued", None).await;
    let working = session(
        &pg,
        "board",
        &tasks[1],
        "running",
        Some(json!({ "turn": 3 })),
    )
    .await;

    let path = format!("/api/boards/{board}/work-sessions");
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let sessions = &body["sessions"];
    assert_eq!(sessions[&tasks[0]]["runId"], queued, "{body}");
    assert_eq!(sessions[&tasks[0]]["turn"], Value::Null, "{body}");
    assert_eq!(sessions[&tasks[1]]["runId"], working, "{body}");
    assert_eq!(sessions[&tasks[1]]["turn"], 3, "{body}");
    sweep(&pg, "board").await;
}
