// /api/conversations/{id}/schedule.
// POST { name, schedule, prompt } → a standing cron on this thread's agent
// whose turns come back INTO this thread. The fleet's own cron door
// (/api/fleet/agents/{id}/crons) creates the same job and reports nowhere;
// this one records the conversation that is waiting on it, so the finished
// turn lands as a message through /api/agents/cron-output.
//
// TWO GATES, asking different questions. The conversation gate is "may you
// see this thread", and it answers 404 rather than 403 — a thread you cannot
// reach must not be distinguishable from one that does not exist, the house
// rule on this plane. The agent gate is "may you change this agent", keyed by
// the model the thread names: a schedule writes to the agent's own scheduler
// and spends its budget on a timer, so being able to read a thread is not
// enough to make its agent work while nobody is watching.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_crons::schedule_from_conversation;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, trimmed_string_member};
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal, object_or_400, store_answer};
use talaria_session::{actor_of, require_agent_manager_by_model, require_user};
use talaria_state::AppState;

// doc: Put this conversation's agent on a timer: a cron job on the agent
// doc: itself whose every run posts its answer back into this thread.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // A malformed id answers the same 404 as an unreachable one — the
    // `$1::uuid` bind in the access read would otherwise make Postgres throw.
    if talaria_params::uuid_gate_404(&id).is_some() {
        return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found"));
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found")),
        Err(e) => {
            return Ok(internal(
                "[conversations] access read on schedule failed",
                e,
            ));
        }
    };
    // The house gate by model, with this door's own sentence on the refusal it
    // owns: its wording is about changing an agent, and what is being refused
    // here is scheduling one. Every other answer it can give — no session, a
    // failed read — passes through untouched, so a broken permission read
    // stays a 500 instead of reading as "you may not".
    let user = match require_agent_manager_by_model(&state, &headers, &conv.agent_model).await {
        Ok(u) => u,
        Err(res) if res.status() == StatusCode::FORBIDDEN => {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "only this agent's managers can schedule it",
            ));
        }
        Err(res) => return Err(res),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // The same bounds the fleet door takes, because it is the same job.
    let name = match trimmed_string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let schedule = match trimmed_string_member(obj, "schedule", 1, 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The agent's own cap: Hermes's /api/jobs refuses a longer prompt
    // (talaria_agent_crons::MAX_PROMPT_CHARS).
    let prompt = match trimmed_string_member(obj, "prompt", 1, 5_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match schedule_from_conversation(&state.pg, &id, &user.id, &name, &schedule, &prompt).await
        {
            Ok(job_id) => {
                // The same action the fleet door logs, so one audit filter
                // finds every cron anyone created; the target is the THREAD,
                // which is the thing that gained a schedule, and the agent
                // rides along in the after block. The prompt does not: it is
                // somebody's words to their own agent, and the job itself is
                // where they are readable.
                let actor = actor_of(&user);
                let after = json!({
                    "name": name,
                    "schedule": schedule,
                    "agentModel": conv.agent_model,
                    "jobId": job_id,
                });
                let target = id.clone();
                let pg = state.pg.clone();
                tokio::spawn(async move {
                    log_audit(
                        &pg,
                        AuditEntry {
                            actor: &actor,
                            action: "cron.create",
                            target_type: "conversation",
                            target_id: Some(&target),
                            target_label: None,
                            before: None,
                            after: Some(after),
                        },
                    )
                    .await;
                });
                Json(json!({ "ok": true, "jobId": job_id })).into_response()
            }
            // Hermes is the validator for the schedule's own grammar and the
            // frequency floor, and its refusal is the one worth reading.
            Err(e) => store_answer(StatusCode::BAD_REQUEST, &e),
        },
    )
}
