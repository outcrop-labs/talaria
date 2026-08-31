// /api/tasks/{id}/usage — port of ui/src/routes/api/tasks.$id.usage.ts.
// Per-ticket token spend. POST (agents, via MCP log_usage): report tokens
// burned working this ticket — attributed to the agent's serving endpoint
// and priced like every other ledger row. GET: the rollup shown on the
// ticket.

use crate::agent_auth::{AgentSubject, agent_caller, require_agent};
use crate::boards::{board_allows_agent, board_role};
use crate::body::{
    NumKind, as_object, nullish_member, number_member, optional_boolean_member,
    optional_max_string_member, parse,
};
use crate::error::{house_error, thrown_internal_error};
use crate::gateway::usage::{TokenCounts, UsageInput, record_usage, task_usage};
use crate::session::require_user;
use crate::state::AppState;
use crate::tasks::{AgentIntent, AgentWriteTarget, agent_ticket_refusal, get_task, log_activity};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

/// This route gates its id with a 404, not the house uuid_gate's 500:
/// agents pass taskId verbatim, and "not found" is the honest answer for a
/// malformed one — not a server fault.
fn uuid_404(id: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    Some(house_error(StatusCode::NOT_FOUND, "not found"))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Some(gate) = uuid_404(&id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on GET usage failed: {e}");
            return thrown_internal_error();
        }
    };
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return gate,
    };
    if let Some(caller) = caller {
        // The CALLER, not its model — the elevated bypass inside board policy
        // is org-wide reach, and a legacy caller only asserted its name.
        let allowed = match board_allows_agent(
            &state.pg,
            &task.board_id,
            &AgentSubject::Caller(caller),
        )
        .await
        {
            Ok(a) => a,
            Err(e) => {
                tracing::error!("[tasks] agent policy read on GET usage failed: {e}");
                return thrown_internal_error();
            }
        };
        if !allowed {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        match board_role(&state.pg, &user.id, &task.board_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
            Err(e) => {
                tracing::error!("[tasks] role read on GET usage failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    match task_usage(&state.pg, &id).await {
        Ok(usage) => Json(usage).into_response(),
        Err(e) => {
            tracing::error!("[tasks] usage rollup failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Some(gate) = uuid_404(&id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST usage failed: {e}");
            return thrown_internal_error();
        }
    };
    // Usage is agent-reported (agents know what they burned); humans don't
    // post token counts by hand.
    let poster = match require_agent(&state.pg, &headers).await {
        Ok(p) => p,
        Err(gate) => return gate,
    };
    let name = poster.model.clone();
    let allowed = match board_allows_agent(
        &state.pg,
        &task.board_id,
        &AgentSubject::Caller(poster.clone()),
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[tasks] agent policy read on POST usage failed: {e}");
            return thrown_internal_error();
        }
    };
    if !allowed {
        return house_error(
            StatusCode::FORBIDDEN,
            &format!("agent \"{name}\" is not allowed on this board"),
        );
    }
    // Work a person has taken off the table takes no more cost and no more
    // activity lines. This route never reaches update_task — it writes a
    // spend row keyed to the ticket AND an activity line onto it — so it
    // asks the SAME predicate agent_safe_patch asks, imported, not copied:
    // closed, archived ticket, archived board, all three.
    let target = AgentWriteTarget {
        board_id: task.board_id.clone(),
        status: task.status.clone(),
        archived_at: task.archived_at.clone(),
    };
    match agent_ticket_refusal(
        &state.pg,
        &target,
        &AgentSubject::Caller(poster),
        AgentIntent::Write,
    )
    .await
    {
        Ok(None) => {}
        Ok(Some(shut)) => {
            return house_error(
                StatusCode::FORBIDDEN,
                &format!("{shut}. No further spend attaches to it."),
            );
        }
        Err(e) => {
            tracing::error!("[tasks] agent authority on POST usage failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let prompt_tokens = match number_member(obj, "promptTokens", NumKind::Int, 0.0, 100_000_000.0) {
        Ok(v) => v as i64,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let completion_tokens =
        match number_member(obj, "completionTokens", NumKind::Int, 0.0, 100_000_000.0) {
            Ok(v) => v as i64,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
    // Model tier the work ran on (alias name); defaults to the agent's main.
    let tier = match nullish_member(obj, "tier", |o, k| optional_max_string_member(o, k, 60)) {
        Ok(v) => v.flatten(),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let estimated = match optional_boolean_member(obj, "estimated") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // A tier must be one of the agent's real alias names — reject typos and
    // routed-model ids loudly instead of silently recording an
    // unattributable (and therefore unpriceable) row. TS's truthiness check
    // also means an EMPTY string tier skips validation entirely and records
    // as '' — preserved, not "fixed".
    if let Some(t) = tier.as_deref().filter(|t| !t.is_empty()) {
        let routed = crate::fleet::routed_model_for(&state.pg, &name, Some(t)).await;
        let known = matches!(routed, Ok(Some(_)));
        if !known {
            return house_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown tier \"{t}\" for {name} — use an alias name or omit"),
            );
        }
    }
    let input = UsageInput {
        agent_model: &name,
        source: "ticket",
        ref_id: Some(&id),
        task_id: Some(&id),
        tier: tier.as_deref(),
        counts: TokenCounts {
            prompt_tokens,
            completion_tokens,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            reasoning_tokens: 0,
        },
        estimated: estimated.unwrap_or(false),
    };
    if let Err(e) = record_usage(&state.pg, &input).await {
        tracing::error!("[tasks] usage record failed: {e}");
        return thrown_internal_error();
    }
    let total = prompt_tokens + completion_tokens;
    if let Err(e) = log_activity(
        &state.pg,
        &id,
        &name,
        "usage",
        &format!("logged {} tokens", crate::gateway::budget::group(total)),
    )
    .await
    {
        tracing::error!("[tasks] usage activity line failed: {e}");
        return thrown_internal_error();
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}
