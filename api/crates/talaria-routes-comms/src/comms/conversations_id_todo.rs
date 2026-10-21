// GET /api/conversations/{id}/todo — the agent's plan for this thread.
//
// THE READ DOOR, gated on the conversation's own access rule (the owner, or a
// member of the plan / work session) — the same predicate every other
// conversation-scoped route uses. The WRITE door is `PUT /api/agent/todo`,
// agent-key only and with no id at all, because the agent writes to whichever
// thread it is mid-turn in.
//
// Inaccessible and nonexistent answer the same 403, never leaking which — the
// house rule on this plane.
//
// AN EMPTY PLAN IS A 200, not a 404. Most threads have no plan and never will,
// so the surface asks this on every thread it opens; a 404 there would make an
// ordinary answer look like a failure and put an error in somebody's console
// for every chat they read.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_todo::{items_json, load, summary};
use talaria_conversations::conversation_accessible;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: The steps the agent is working through on this conversation.

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[todo] access read failed", e)),
    }
    match load(&state.pg, &id).await {
        Ok(items) => Ok(Json(json!({
            "items": items_json(&items),
            "summary": summary(&items),
        }))
        .into_response()),
        Err(e) => Ok(internal("[todo] load failed", e)),
    }
}
