// GET /api/agents/{id}/conversations ({id} is the agent's model). The profile
// drawer's "Conversations" list for an agent: the comms rooms the caller is
// in that seat this agent, merged with the caller's own agent-DM threads with
// it (kind 'agent', named by the thread title or "New thread"), newest
// activity first. Answers { conversations: [{ id, kind, name, lastAt,
// unreadCount }] }. An agent the caller may not see under GET /api/agents'
// gate (someone else's personal assistant, outside their access list) is a
// 404 — the same answer as no such agent.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::usable_agent_gate;
use talaria_channels::list_shared_with_agent;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(model): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => return Ok(internal("[agents/conversations] access read failed", e)),
    };
    if model.is_empty() || !gate(&model) {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    Ok(
        match list_shared_with_agent(&state.pg, &user.id, &model).await {
            Ok(conversations) => Json(json!({ "conversations": conversations })).into_response(),
            Err(e) => internal("[agents/conversations] read failed", e),
        },
    )
}
