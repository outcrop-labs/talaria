// GET /api/agents. The fleet the current user may use (definition-backed
// agents with their model tiers, filtered by per-agent access). Auth-gated.

use crate::error::thrown_internal_error;
use crate::fleet::{list_fleet_agents, usable_agent_gate};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct AgentsBody {
    agents: Vec<crate::fleet::FleetAgentEntry>,
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Owner-aware: a personal assistant is only visible to its owner.
    let (agents, gate) = tokio::join!(
        list_fleet_agents(&state.pg),
        usable_agent_gate(&state.pg, &user.id, &user.role),
    );
    let (agents, gate) = match (agents, gate) {
        (Ok(a), Ok(g)) => (a, g),
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!("[agents] fleet read failed: {e}");
            return thrown_internal_error();
        }
    };
    let visible = agents.into_iter().filter(|a| gate(&a.agent.id)).collect();
    Json(AgentsBody { agents: visible }).into_response()
}
