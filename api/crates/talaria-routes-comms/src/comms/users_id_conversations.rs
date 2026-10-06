// GET /api/users/{id}/conversations. The profile drawer's "Conversations"
// list and the "all conversations with <person>" view: every comms room —
// DM, channel, relay — the caller and that person are both in right now,
// newest activity first. Never archived rooms, never a ticket's task room.
// Answers { conversations: [{ id, kind, name, lastAt, unreadCount }] }, where
// `name` is the other person's for a DM and `unreadCount` is the caller's.
// A malformed or unknown id is a 404; asking about yourself lists your rooms.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_channels::list_shared_with_person;
use talaria_error::{house_error, internal};
use talaria_params::uuid_gate_404;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_users::profile_face;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = uuid_gate_404(&id) {
        return Ok(gate);
    }
    match profile_face(&state.pg, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[users/conversations] person read failed", e)),
    }
    Ok(
        match list_shared_with_person(&state.pg, &user.id, &id).await {
            Ok(conversations) => Json(json!({ "conversations": conversations })).into_response(),
            Err(e) => internal("[users/conversations] read failed", e),
        },
    )
}
