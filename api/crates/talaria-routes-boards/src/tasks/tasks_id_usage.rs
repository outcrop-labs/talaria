// GET /api/tasks/{id}/usage. Per-ticket token spend: the rollup shown on the
// ticket, read from the `usage_events` rows that carry this ticket's id — the
// work-session flow's metered rows, priced like every other ledger row.
//
// THERE IS NO POST ANY MORE. Agents used to self-report spend here through
// the MCP `log_usage` tool. Every Hermes agent's model traffic already goes
// through the gateway and is metered there, so a self-report counted the same
// spend twice — in the ticket rollup, the fleet totals and the agent's own
// budget window — on counts the agent guessed. Rows written before the tool
// went (`source = 'ticket'`) still roll up here.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_api_facades::gateway::usage::task_usage;
use talaria_boards::{board_allows_agent, board_role};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_tasks::get_task;

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
