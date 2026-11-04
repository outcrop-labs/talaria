// Live-DB proof of the snapshot frame on the tool-events door (cargo test
// -- --ignored): a `kind: "snapshot"` POST from the agent's plugin becomes
// one step row in the live work session's ledger, keyed by the batch's last
// call, bound to the agent's own job, placed at the turn the run is on. The
// door's refusals are the point: a session that is no live run of this
// agent lands nowhere, a job that is not this agent's makes the step
// unrestorable, a frame the ledger holds adds no row, and an unkeyed caller
// is turned away. House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it tool_snapshots_live:: -- --ignored

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::runs::store::run_checkpoints;
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

use crate::support::live_app_state;

const PREFIX: &str = "tool-snapshots-live";

async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from runs where kind = 'work-session' and input->>'agentModel' like $1")
        .bind(format!("{PREFIX}-{tag}-%"))
        .execute(pg)
        .await
        .unwrap();
}

/// An agent with its own minted credential. Returns (def id, model, key).
async fn agent(state: &AppState, tag: &str) -> (String, String, String) {
    let slug = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let model = format!("{slug}-model");
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'personal', $2, 'Tool Snapshots Live') returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, &id)
        .await
        .expect("agent key mints");
    (id, model, key)
}

/// A started job of the agent: the thing a snapshot's `jobId` must name.
/// It goes with the def.
async fn started_job(pg: &PgPool, def_id: &str, model: &str) -> String {
    sqlx::query_scalar(
        "insert into workbench_jobs \
           (agent_id, agent_model, task_id, host_id, repo, branch, effort, plan, status) \
         values ($1::uuid, $2, null, 'github', 'acme/widgets', 'agent/snap', 'standard', '', \
                 'started') \
         returning id::text",
    )
    .bind(def_id)
    .bind(model)
    .fetch_one(pg)
    .await
    .unwrap()
}

/// A live work session of the agent, mid-turn.
async fn live_run(pg: &PgPool, model: &str, turn: i64) -> String {
    sqlx::query_scalar(
        "insert into runs (kind, state, input, checkpoint) \
         values ('work-session', 'running', $1, $2) returning id::text",
    )
    .bind(json!({ "agentModel": model }))
    .bind(json!({ "stage": "send", "turn": turn, "stageAttempt": 0 }))
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn post(state: &AppState, key: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/agents/tool-events")
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("x-api-key", key);
    }
    let res = talaria_api::routes::router(state.clone())
        .oneshot(builder.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn frame(session: &str, job: Option<&str>, calls: Value) -> Value {
    json!({
        "kind": "snapshot",
        "sessionId": session,
        "turnId": "hermes-turn-4",
        "apiRequestId": "hermes-turn-4:api:2",
        "calls": calls,
        "jobId": job,
        "tree": "0123456789abcdef0123456789abcdef01234567",
        "commit": "fedcba9876543210fedcba9876543210fedcba98",
        "at": 1_700_000_000_000i64,
        "ms": 24,
    })
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_snapshot_lands_one_step_row_on_the_live_run_and_only_once() {
    let state = live_app_state().await;
    let tag = "lands";
    cleanup(&state.pg, tag).await;
    let (def_id, model, key) = agent(&state, tag).await;
    let job = started_job(&state.pg, &def_id, &model).await;
    let run = live_run(&state.pg, &model, 3).await;
    let batch = json!([
        {"id": "call_a", "name": "terminal", "status": "completed"},
        {"id": "call_b", "name": "mcp__workbench__job_status", "status": "completed"},
        {"id": "call_c", "name": "terminal", "status": "error"},
    ]);

    let (status, body) = post(&state, Some(&key), frame(&run, Some(&job), batch.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["landed"], true);
    assert_eq!(body["seq"], 1);

    let (status, body) = post(&state, Some(&key), frame(&run, Some(&job), batch)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["landed"], true);
    assert_eq!(body["duplicate"], true, "{body}");

    let rows = run_checkpoints(&state.pg, &run).await.unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.tool_call_id.as_deref(), Some("call_c"));
    assert_eq!(row.tool.as_deref(), Some("terminal"));
    assert_eq!(
        row.title.as_deref(),
        Some("3 calls: terminal ×2, mcp__workbench__job_status")
    );
    assert_eq!(row.status.as_deref(), Some("failed"));
    assert_eq!(row.turn, Some(3), "the turn the run is on");
    assert_eq!(row.step_no, Some(1), "numbered on arrival");
    assert_eq!(row.agent_turn_id.as_deref(), Some("hermes-turn-4"));
    assert_eq!(row.job_id.as_deref(), Some(job.as_str()));
    assert_eq!(
        row.workspace_commit.as_deref(),
        Some("fedcba9876543210fedcba9876543210fedcba98")
    );
    assert!(row.restorable, "{row:?}");
    assert_eq!(row.created_at, "2023-11-14T22:13:20.000Z");

    cleanup(&state.pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_frame_lands_nowhere_without_a_live_run_and_a_stranger_s_job_makes_it_unrestorable() {
    let state = live_app_state().await;
    let tag = "refusals";
    cleanup(&state.pg, tag).await;
    let (def_id, model, key) = agent(&state, tag).await;
    let (other_def, other_model, _) = agent(&state, tag).await;
    let own_job = started_job(&state.pg, &def_id, &model).await;
    let other_job = started_job(&state.pg, &other_def, &other_model).await;
    let calls = json!([{"id": "call_x", "name": "terminal"}]);

    // No run named: a session id that is not a live work session of this agent.
    let nobody = uuid::Uuid::new_v4().to_string();
    let (status, body) = post(
        &state,
        Some(&key),
        frame(&nobody, Some(&own_job), calls.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["landed"], false, "{body}");
    assert!(
        run_checkpoints(&state.pg, &nobody)
            .await
            .unwrap()
            .is_empty()
    );

    // Another agent's live run is not this agent's either.
    let theirs = live_run(&state.pg, &other_model, 1).await;
    let (_, body) = post(
        &state,
        Some(&key),
        frame(&theirs, Some(&own_job), calls.clone()),
    )
    .await;
    assert_eq!(body["landed"], false, "{body}");
    assert!(
        run_checkpoints(&state.pg, &theirs)
            .await
            .unwrap()
            .is_empty()
    );

    // A run of this agent that ended a while ago names nothing.
    let long_done = live_run(&state.pg, &model, 2).await;
    sqlx::query(
        "update runs set state = 'done', finished_at = now() - interval '20 minutes' \
         where id = $1::uuid",
    )
    .bind(&long_done)
    .execute(&state.pg)
    .await
    .unwrap();
    let (_, body) = post(
        &state,
        Some(&key),
        frame(&long_done, Some(&own_job), calls.clone()),
    )
    .await;
    assert_eq!(body["landed"], false, "{body}");

    // The job named is not this agent's: the row lands, and says it cannot restore.
    let run = live_run(&state.pg, &model, 1).await;
    let (_, body) = post(
        &state,
        Some(&key),
        frame(&run, Some(&other_job), calls.clone()),
    )
    .await;
    assert_eq!(body["landed"], true, "{body}");
    let rows = run_checkpoints(&state.pg, &run).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].job_id, None);
    assert!(!rows[0].restorable);
    assert_eq!(
        rows[0].unrestorable_reason.as_deref(),
        Some(format!("job {other_job} is not this agent's").as_str())
    );

    // No calls is no frame; no key is no caller.
    let (status, _) = post(
        &state,
        Some(&key),
        json!({"kind": "snapshot", "sessionId": run}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = post(&state, None, frame(&run, Some(&own_job), calls)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    cleanup(&state.pg, tag).await;
}

/// The step after a turn's last tool call reaches the door after the run has
/// finished: it still lands, placed at the last closed turn after that turn's
/// last step, so the run's final state has a rewind point too.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_last_step_lands_after_the_run_has_ended_and_is_placed() {
    let state = live_app_state().await;
    let tag = "last-step";
    cleanup(&state.pg, tag).await;
    let (def_id, model, key) = agent(&state, tag).await;
    let job = started_job(&state.pg, &def_id, &model).await;
    let run = live_run(&state.pg, &model, 1).await;
    let first = frame(
        &run,
        Some(&job),
        json!([{"id": "call_1", "name": "terminal"}]),
    );
    let (_, body) = post(&state, Some(&key), first).await;
    assert_eq!(body["seq"], 1, "{body}");
    sqlx::query(
        "insert into run_checkpoints (run_id, seq, kind, turn, status, title) \
         values ($1::uuid, 2, 'turn', 1, 'record', 'turn 1 answered')",
    )
    .bind(&run)
    .execute(&state.pg)
    .await
    .unwrap();
    sqlx::query(
        "update runs set state = 'done', finished_at = now(), checkpoint_seq = 2, \
                checkpoint = '{\"stage\":\"send\",\"turn\":2,\"stageAttempt\":0}' \
         where id = $1::uuid",
    )
    .bind(&run)
    .execute(&state.pg)
    .await
    .unwrap();

    let late = frame(
        &run,
        Some(&job),
        json!([{"id": "call_2", "name": "mcp__talaria__report_outcome"}]),
    );
    let (status, body) = post(&state, Some(&key), late).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["landed"], true, "{body}");
    assert_eq!(body["seq"], 3);
    let rows = run_checkpoints(&state.pg, &run).await.unwrap();
    let last = rows
        .iter()
        .find(|r| r.tool_call_id.as_deref() == Some("call_2"))
        .unwrap();
    assert_eq!((last.turn, last.step_no), (Some(1), Some(2)), "{last:?}");
    // Through the real door there is no container to read the transcript
    // from, so the late row makes no promise; the stubbed read below proves
    // the held case.
    assert!(!last.restorable);
    assert!(
        last.unrestorable_reason
            .as_deref()
            .unwrap()
            .contains("could not be read"),
        "{last:?}"
    );
    assert_eq!(
        rows.iter().map(|r| r.seq).collect::<Vec<_>>(),
        [1, 3, 2],
        "the late step reads before the record row that closed its turn"
    );

    cleanup(&state.pg, tag).await;
}

/// Removing a job's workdir retires its steps and marks the job cleared in
/// one transaction: a sweep can never find a job it will not scan again
/// whose rows still promise a restore.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_removed_workdir_retires_its_steps_with_the_job_s_done_marker() {
    let state = live_app_state().await;
    let tag = "removed";
    cleanup(&state.pg, tag).await;
    let (def_id, model, key) = agent(&state, tag).await;
    let job = started_job(&state.pg, &def_id, &model).await;
    let run = live_run(&state.pg, &model, 1).await;
    let first = frame(
        &run,
        Some(&job),
        json!([{"id": "call_1", "name": "terminal"}]),
    );
    let (_, body) = post(&state, Some(&key), first).await;
    assert_eq!(body["landed"], true, "{body}");

    talaria_workbench_mcp::teardown::mark_removed(&state.pg, &job)
        .await
        .unwrap();
    let rows = run_checkpoints(&state.pg, &run).await.unwrap();
    assert!(!rows[0].restorable);
    assert_eq!(
        rows[0].unrestorable_reason.as_deref(),
        Some("the job's checkout was removed")
    );
    let cleared: bool = sqlx::query_scalar(
        "select workspace_cleared_at is not null from workbench_jobs where id = $1::uuid",
    )
    .bind(&job)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    assert!(cleared);

    cleanup(&state.pg, tag).await;
}

/// A late frame is checked against the run's own transcript on arrival, as a
/// live one is at the next record stage: a boundary the transcript holds is
/// restorable, one it does not hold is not, and a transcript that cannot be
/// read makes no promise.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_late_frame_is_restorable_only_when_the_run_s_transcript_holds_its_call() {
    use futures_util::FutureExt as _;
    use talaria_routes_fleet::agents::tool_snapshots::{TranscriptRead, land_with};
    let state = live_app_state().await;
    let tag = "late-verify";
    cleanup(&state.pg, tag).await;
    let (def_id, model, key) = agent(&state, tag).await;
    let _ = key;
    let job = started_job(&state.pg, &def_id, &model).await;
    let run = live_run(&state.pg, &model, 1).await;
    sqlx::query(
        "update runs set state = 'done', finished_at = now(), \
                checkpoint = '{\"stage\":\"send\",\"turn\":2,\"stageAttempt\":0}' \
         where id = $1::uuid",
    )
    .bind(&run)
    .execute(&state.pg)
    .await
    .unwrap();
    let caller = talaria_api::agent_auth::AgentCaller {
        id: Some(def_id.clone()),
        model: model.clone(),
        legacy: false,
    };
    let held: TranscriptRead = std::sync::Arc::new(|_model, _session| {
        async { Some(vec!["call_held".to_string()]) }.boxed()
    });
    let unreadable: TranscriptRead = std::sync::Arc::new(|_model, _session| async { None }.boxed());

    let f = |call: &str| frame(&run, Some(&job), json!([{"id": call, "name": "terminal"}]));
    land_with(&state, &caller, f("call_held"), &held)
        .await
        .unwrap();
    land_with(&state, &caller, f("call_lost"), &held)
        .await
        .unwrap();
    land_with(&state, &caller, f("call_blind"), &unreadable)
        .await
        .unwrap();
    let rows = run_checkpoints(&state.pg, &run).await.unwrap();
    let by = |call: &str| {
        rows.iter()
            .find(|r| r.tool_call_id.as_deref() == Some(call))
            .unwrap()
    };
    assert!(by("call_held").restorable, "{:?}", by("call_held"));
    assert!(!by("call_lost").restorable);
    assert!(
        by("call_lost")
            .unrestorable_reason
            .as_deref()
            .unwrap()
            .starts_with("not in this run's transcript"),
        "{:?}",
        by("call_lost")
    );
    assert!(!by("call_blind").restorable);
    assert!(
        by("call_blind")
            .unrestorable_reason
            .as_deref()
            .unwrap()
            .contains("could not be read"),
        "{:?}",
        by("call_blind")
    );
    // A frame whose turn is another task's is not a step of this run at all.
    let mut other = f("call_other");
    other["turnId"] = json!(format!(
        "{run}:7991ecb1-0000-4000-8000-000000000000:309793a7"
    ));
    land_with(&state, &caller, other, &held).await.unwrap();
    assert!(rows.len() == 3 && run_checkpoints(&state.pg, &run).await.unwrap().len() == 3);

    cleanup(&state.pg, tag).await;
}
