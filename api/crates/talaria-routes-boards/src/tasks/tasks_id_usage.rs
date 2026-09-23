// /api/tasks/{id}/usage. Per-ticket token spend. POST (agents, via MCP
// log_usage): report tokens burned working this ticket — attributed to the
// agent's serving endpoint and priced like every other ledger row. GET: the
// rollup shown on the ticket.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_agent_auth::{AgentSubject, agent_caller, require_agent};
use talaria_api_facades::gateway::usage::{TokenCounts, UsageInput, record_usage, task_usage};
use talaria_boards::{board_allows_agent, board_role};
use talaria_body::{
    NumKind, nullish_member, number_member, optional_boolean_member, optional_max_string_member,
    parse,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_tasks::{AgentIntent, AgentWriteTarget, agent_ticket_refusal, get_task, log_activity};

// Ticket address is a uuid or the board ref (`PLAT-118`). A miss is 404,
// never the house uuid_gate 500 — agents pass the ref the assignment showed.

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[tasks] read on GET usage failed", e)),
    };
    let caller = agent_caller(&state.pg, &headers).await?;
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
            Err(e) => return Ok(internal("[tasks] agent policy read on GET usage failed", e)),
        };
        if !allowed {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
    } else {
        let user = require_user(&state, &headers).await?;
        match board_role(&state.pg, &user.id, &task.board_id).await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
            Err(e) => return Ok(internal("[tasks] role read on GET usage failed", e)),
        }
    }
    Ok(match task_usage(&state.pg, &id).await {
        Ok(usage) => Json(usage).into_response(),
        Err(e) => internal("[tasks] usage rollup failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[tasks] read on POST usage failed", e)),
    };
    // Usage is agent-reported (agents know what they burned); humans don't
    // post token counts by hand.
    let poster = match require_agent(&state.pg, &headers).await {
        Ok(p) => p,
        Err(gate) => return Ok(gate),
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
            return Ok(internal(
                "[tasks] agent policy read on POST usage failed",
                e,
            ));
        }
    };
    if !allowed {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            &format!("agent \"{name}\" is not allowed on this board"),
        ));
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
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                &format!("{shut}. No further spend attaches to it."),
            ));
        }
        Err(e) => return Ok(internal("[tasks] agent authority on POST usage failed", e)),
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let prompt_tokens = match number_member(obj, "promptTokens", NumKind::Int, 0.0, 100_000_000.0) {
        Ok(v) => v as i64,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let completion_tokens =
        match number_member(obj, "completionTokens", NumKind::Int, 0.0, 100_000_000.0) {
            Ok(v) => v as i64,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        };
    // Model tier the work ran on (alias name); defaults to the agent's main.
    let tier = match nullish_member(obj, "tier", |o, k| optional_max_string_member(o, k, 60)) {
        Ok(v) => v.flatten(),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let estimated = match optional_boolean_member(obj, "estimated") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // A tier must be one of the agent's real alias names — reject typos and
    // routed-model ids loudly instead of silently recording an
    // unattributable (and therefore unpriceable) row. An EMPTY string tier
    // skips validation entirely and records as '' — preserved, not "fixed".
    if let Some(t) = tier.as_deref().filter(|t| !t.is_empty()) {
        let routed = talaria_api_facades::fleet::routed_model_for(&state.pg, &name, Some(t)).await;
        let known = matches!(routed, Ok(Some(_)));
        if !known {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown tier \"{t}\" for {name} — use an alias name or omit"),
            ));
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
        return Ok(internal("[tasks] usage record failed", e));
    }
    let total = prompt_tokens + completion_tokens;
    if let Err(e) = log_activity(
        &state.pg,
        &id,
        &name,
        "usage",
        &format!(
            "logged {} tokens",
            talaria_api_facades::gateway::budget::group(total)
        ),
    )
    .await
    {
        return Ok(internal("[tasks] usage activity line failed", e));
    }
    Ok(Json(serde_json::json!({ "ok": true })).into_response())
}
