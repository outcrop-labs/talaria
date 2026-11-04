// /api/conversations/{id}/unread.
// POST → mark the thread unread for the caller: their read cursor steps back
// to just before the thread's newest turn that counts for them (the agent's,
// or another person's), so its unread pill reads 1 again. → { ok,
// unreadCount }; unreadCount is 0, and nothing is written, when the thread
// holds nothing that could count. The read route's twin, gated by the same
// access rule (the owner, or a member of the plan). It touches only the
// caller's own cursor row: the bell's rows stay as they are.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_conversations::{conversation_accessible, mark_conversation_unread};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    // Inaccessible and nonexistent answer the same 403, as the read does.
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[conversations] access read on unread failed", e)),
    }
    Ok(
        match mark_conversation_unread(&state.pg, &id, &user.id).await {
            Ok(marked) => Json(json!({ "ok": true, "marked": marked })).into_response(),
            Err(e) => internal("[conversations] mark unread failed", e),
        },
    )
}
