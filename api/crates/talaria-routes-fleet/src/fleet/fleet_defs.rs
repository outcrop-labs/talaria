// GET /api/fleet/defs. The harness registry: agent definitions (latest
// version inline) + LLM endpoints + brain routability. Admins only — the
// config surface includes infra layout.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_defs::list_agent_defs_wire;
use talaria_api_facades::gateway::registry::list_endpoints_wire;
use talaria_brain_health::fleet_brain_health;
use talaria_error::internal;
use talaria_session::require_perm;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let defs = match list_agent_defs_wire(&state.pg).await {
        Ok(d) => d,
        Err(e) => return internal("[fleet] list_agent_defs_wire failed", e),
    };
    let endpoints = match list_endpoints_wire(&state.pg).await {
        Ok(e) => e,
        Err(e) => return internal("[fleet] list_endpoints_wire failed", e),
    };
    let brains = match fleet_brain_health(&state.pg).await {
        Ok(b) => b,
        Err(e) => return internal("[fleet] fleet_brain_health failed", e),
    };
    // The endpoints' prices ride as numbers — a whole $/MTok price prints the
    // JS way (`3`, never `3.0`).
    let mut body = json!({ "defs": defs, "endpoints": endpoints, "brains": brains });
    talaria_body::js_numberify(&mut body);
    Json(body).into_response()
}
