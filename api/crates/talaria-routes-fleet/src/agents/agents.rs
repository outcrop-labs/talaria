// GET /api/agents. The fleet the current user may use (definition-backed
// agents with their model tiers, filtered by per-agent access). Auth-gated.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use talaria_api_facades::fleet::{list_fleet_agents, usable_agent_gate};
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

#[derive(serde::Serialize)]
struct AgentsBody {
    agents: Vec<talaria_api_facades::fleet::FleetAgentEntry>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // Owner-aware: a personal assistant is only visible to its owner.
    let (agents, gate) = tokio::join!(
        list_fleet_agents(&state.pg),
        usable_agent_gate(&state.pg, &user.id, &user.role),
    );
    let (agents, gate) = match (agents, gate) {
        (Ok(a), Ok(g)) => (a, g),
        (Err(e), _) | (_, Err(e)) => return Ok(internal("[agents] fleet read failed", e)),
    };
    let visible = agents.into_iter().filter(|a| gate(&a.agent.id)).collect();
    Ok(Json(AgentsBody { agents: visible }).into_response())
}
