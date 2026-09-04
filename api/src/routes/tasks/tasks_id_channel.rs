// /api/tasks/{id}/channel.
//
// OPEN THE ROOM FOR A TICKET, creating it the first time.
//
// ON DEMAND, like the conversation door this replaces: most tickets are read
// and moved, never discussed, and a room per task would be a channel nobody
// said anything in. The first person to open the Discussion tab is what
// makes it.
//
// ACCESS IS THE BOARD'S. `board_role` is the same check the ticket itself
// goes through — a member of the task's board, that is all. Deliberately
// not a second permission: two answers to "may this person see this task"
// is how they drift. Agents do not come here: they reach the room through
// the comment write and the mention grammar, which have their own agent
// predicates; this is the human door the Discussion tab opens.
//
// Idempotent by construction: an existing link short-circuits the ensure,
// so opening the tab twice returns the same room, races included.

use crate::boards::board_role;
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use crate::tasks::{ensure_task_channel, get_task};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("task", "POST channel", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST channel failed: {e}");
            return thrown_internal_error();
        }
    };
    match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] role read on POST channel failed: {e}");
            return thrown_internal_error();
        }
    }
    match ensure_task_channel(&state.pg, &id).await {
        Ok(Some(channel_id)) => Json(json!({ "channelId": channel_id })).into_response(),
        // Nobody could hold the row — the ladder walked off the edge of an
        // empty board. Saying so beats a 500; adding a board member fixes
        // it.
        Ok(None) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "this ticket has no owner to hold its room — add someone to its board" })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[tasks] channel ensure failed: {e}");
            thrown_internal_error()
        }
    }
}
