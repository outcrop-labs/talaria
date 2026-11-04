// POST /api/runs/{id}/rewind. Start a rewind of a work session to one of its
// ledger steps, as a `run-rewind` run of its own. The answer is that run's
// id: the rewind runs detached, stage by stage, and its row says how it went.
// A refusal this route can see without touching anything (no such step, a
// step with no snapshot, a rewind already running for this session) answers
// here, so a person is not sent to watch a run that was never going to start.
//
// GATED LIKE THE STOP ROUTE: owner or editor of the ticket's board. A rewind
// ends the agent's current work and moves the ticket, which a viewer may
// watch and not do.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_api_facades::runs::run::{EnqueueOptions, enqueue};
use talaria_api_facades::runs::store::{RunStore, run_checkpoints};
use talaria_boards::{board_role, can_edit};
use talaria_error::{house_error, internal};
use talaria_runs_rewind::{REWIND_KIND, RewindInput, rewind_run};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&run_id) {
        return Ok(gate);
    }
    let Some(seq) = body.get("seq").and_then(Value::as_i64) else {
        return Ok(house_error(StatusCode::BAD_REQUEST, "seq is required"));
    };
    let store = talaria_api_facades::runs::store::PgRunStore::new(state.pg.clone());
    let run = match store.get(&run_id).await {
        Ok(Some(run)) if run.kind == "work-session" => run,
        Ok(_) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[runs/rewind] run read failed", e)),
    };
    let (task_id, agent_model, board_id) = match (
        run.input["taskId"].as_str(),
        run.input["agentModel"].as_str(),
        run.input["boardId"].as_str(),
    ) {
        (Some(t), Some(a), Some(b)) => (t.to_string(), a.to_string(), b.to_string()),
        _ => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
    };
    match board_role(&state.pg, &user.id, &board_id).await {
        Ok(role) if can_edit(role.as_deref()) => {}
        Ok(_) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[runs/rewind] role read failed", e)),
    }
    let row = match run_checkpoints(&state.pg, &run_id).await {
        Ok(rows) => rows.into_iter().find(|row| row.seq == seq),
        Err(e) => return Ok(internal("[runs/rewind] ledger read failed", e)),
    };
    let Some(row) = row else {
        return Ok(house_error(StatusCode::NOT_FOUND, "no such step"));
    };
    if !row.restorable {
        return Ok(house_error(
            StatusCode::CONFLICT,
            &format!(
                "this step cannot be restored: {}",
                row.unrestorable_reason
                    .as_deref()
                    .unwrap_or("it has no snapshot")
            ),
        ));
    }
    let live: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "select id::text from runs \
         where kind = $1 and input->>'sourceRunId' = $2 \
           and state in ('queued', 'running', 'awaiting') limit 1",
    )
    .bind(REWIND_KIND)
    .bind(&run_id)
    .fetch_optional(&state.pg)
    .await;
    match live {
        Ok(Some((rewind,))) => {
            return Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": "a rewind of this session is already running", "rewindRunId": rewind })),
            )
                .into_response());
        }
        Ok(None) => {}
        Err(e) => return Ok(internal("[runs/rewind] rewind read failed", e)),
    }
    let Some(redis) = state.redis().await.ok() else {
        return Ok(internal(
            "[runs/rewind] redis unavailable — refusing to start blind",
            "redis connection unavailable",
        ));
    };
    let realtime = talaria_realtime_watch::RealtimeDeps::publish_only(Some(redis.clone()));
    let deps = talaria_api_facades::runs::real_run_deps(state.pg.clone(), redis, realtime);
    let input = RewindInput {
        source_run_id: run_id.clone(),
        seq,
        requested_by: user.id.clone(),
        task_id: task_id.clone(),
        agent_model,
        board_id,
    };
    let opts = EnqueueOptions {
        owner_user_id: Some(user.id.clone()),
        subject_type: Some("task".into()),
        subject_id: Some(task_id),
        phase: Some(format!("rewinding to step {seq}")),
        id: None,
        start: Some(true),
    };
    match enqueue(rewind_run(), json!(input), opts, &deps).await {
        Ok(rewind) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "rewindRunId": rewind.id })),
        )
            .into_response()),
        Err(e) => Ok(internal("[runs/rewind] enqueue failed", e)),
    }
}
