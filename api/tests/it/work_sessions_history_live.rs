// Live-DB proof of GET /api/tasks/{id}/work-sessions: the rows that reach a
// ticket's work log, through the real router. A null `runs.checkpoint` is a
// decode question only a real row can ask, and the lineage columns are a select
// no unit test runs.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it work_sessions_history_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email, sweep_user_rows};

const PREFIX: &str = "work-sessions-history-live";

async fn ticket(pg: &PgPool, owner: &str) -> String {
    let board: String = sqlx::query_scalar(
        "insert into boards (name, owner_id) values ('history live', $1::uuid) returning id::text",
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
    sqlx::query_scalar(
        "insert into tasks (board_id, title) values ($1::uuid, 'history') returning id::text",
    )
    .bind(&board)
    .fetch_one(pg)
    .await
    .unwrap()
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

async fn member(state: &AppState, tag: &str, name: &str) -> (String, String) {
    let email = format!("{name}-{tag}@{PREFIX}.invalid");
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from runs where input->>'agentModel' = $1")
        .bind(model(tag))
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, PREFIX, tag).await;
}

fn row<'a>(body: &'a Value, run_id: &str) -> &'a Value {
    body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["runId"] == run_id)
        .unwrap_or_else(|| panic!("{run_id} is in the log: {body}"))
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_session_with_no_checkpoint_keeps_the_work_log_readable() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "null").await;
    let (owner, cookie) = member(&state, "null", "owner").await;
    let task_id = ticket(&pg, &owner).await;
    let queued = session(&pg, "null", &task_id, "queued", None).await;
    let worked = session(&pg, "null", &task_id, "done", Some(json!({ "turn": 3 }))).await;

    let path = format!("/api/tasks/{task_id}/work-sessions");
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(row(&body, &queued)["turn"], Value::Null);
    assert_eq!(row(&body, &worked)["turn"], 3);
    sweep(&pg, "null").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_log_groups_attempts_and_says_what_each_did_outside() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "lineage").await;
    let (owner, cookie) = member(&state, "lineage", "owner").await;
    let (_, stranger) = member(&state, "lineage", "stranger").await;
    let task_id = ticket(&pg, &owner).await;
    let root = session(
        &pg,
        "lineage",
        &task_id,
        "cancelled",
        Some(json!({ "turn": 2 })),
    )
    .await;
    let other = session(&pg, "lineage", &task_id, "done", Some(json!({ "turn": 1 }))).await;
    let attempt = session(&pg, "lineage", &task_id, "running", None).await;
    sqlx::query(
        "update runs set lineage_id = $2::uuid, attempt_no = 2, \
                rewound_from_run_id = $2::uuid, rewound_from_seq = 4 where id = $1::uuid",
    )
    .bind(&attempt)
    .bind(&root)
    .execute(&pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into usage_events (agent_model, source, ref_id, prompt_tokens, completion_tokens) \
         values ($1, 'chat', $2::uuid, 30, 12)",
    )
    .bind(model("lineage"))
    .bind(&attempt)
    .execute(&pg)
    .await
    .unwrap();

    let path = format!("/api/tasks/{task_id}/work-sessions");
    let (status, body) = call_json(&state, "GET", &path, Some(&cookie), None).await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let first = row(&body, &root);
    assert_eq!(first["attemptNo"], 1);
    assert_eq!(first["lineageId"], json!(root));
    assert_eq!(first["rewoundFromRunId"], Value::Null);
    let second = row(&body, &attempt);
    assert_eq!(second["attemptNo"], 2);
    assert_eq!(second["lineageId"], json!(root));
    assert_eq!(second["rewoundFromRunId"], json!(root));
    assert_eq!(second["rewoundFromSeq"], 4);
    assert_eq!(second["outside"]["tokens"], 42);
    assert_eq!(second["outside"]["pushes"], json!([]));
    let lineages = body["lineages"].as_array().unwrap();
    assert!(
        lineages.contains(&json!({ "lineageId": root, "attempts": [root, attempt] })),
        "{body}"
    );
    assert!(
        lineages.contains(&json!({ "lineageId": other, "attempts": [other] })),
        "{body}"
    );

    let (status, _) = call_json(&state, "GET", &path, Some(&stranger), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    sqlx::query("delete from usage_events where agent_model = $1")
        .bind(model("lineage"))
        .execute(&pg)
        .await
        .unwrap();
    sweep(&pg, "lineage").await;
}
