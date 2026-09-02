// GET /api/fleet/defs. The harness registry: agent definitions (latest
// version inline) + LLM endpoints + brain routability. Admins only — the
// config surface includes infra layout.

use crate::agent_defs::list_agent_defs_wire;
use crate::brain_health::fleet_brain_health;
use crate::error::thrown_internal_error;
use crate::gateway::registry::list_endpoints_wire;
use crate::session::require_perm;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let defs = match list_agent_defs_wire(&state.pg).await {
        Ok(d) => d,
        Err(_) => return thrown_internal_error(),
    };
    let endpoints = match list_endpoints_wire(&state.pg).await {
        Ok(e) => e,
        Err(_) => return thrown_internal_error(),
    };
    let brains = match fleet_brain_health(&state.pg).await {
        Ok(b) => b,
        Err(_) => return thrown_internal_error(),
    };
    // The endpoints' prices ride as numbers — a whole $/MTok price prints the
    // JS way (`3`, never `3.0`).
    let mut body = json!({ "defs": defs, "endpoints": endpoints, "brains": brains });
    crate::body::js_numberify(&mut body);
    Json(body).into_response()
}
