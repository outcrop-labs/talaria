// /api/tasks/{id}/conversation.
//
// OPEN THE THREAD FOR A TICKET, creating it the first time.
//
// ON DEMAND, like the research conversation this mirrors: most tickets are
// read and moved, never discussed, and a conversation row per task would be
// a thread nobody said anything in. The first person to open the Discussion
// tab is what makes it — and the backfill made one for every ticket that
// already had comments, so this route only ever creates threads for silent
// tickets or tickets born after the cutover.
//
// ACCESS IS THE BOARD'S. `board_role` is the same check the ticket itself
// goes through — a member of the task's board, that is all. Deliberately
// not a second permission: two answers to "may this person see this task"
// is how they drift. Agents do not come here: they reach the thread through
// the comment write and the chat door, which have their own agent
// predicates; this is the human door the Discussion tab opens.
//
// Idempotent by construction: an existing link short-circuits the ensure,
// so opening the tab twice returns the same conversation, races included.

use crate::boards::board_role;
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::state::AppState;
use crate::tasks::{ensure_task_conversation, get_task};
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
    if let Some(gate) = crate::params::uuid_gate("task", "POST conversation", &id) {
        return gate;
    }
    let task = match get_task(&state.pg, &id).await {
        Ok(Some(t)) => t,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] read on POST conversation failed: {e}");
            return thrown_internal_error();
        }
    };
    match board_role(&state.pg, &user.id, &task.board_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[tasks] role read on POST conversation failed: {e}");
            return thrown_internal_error();
        }
    }
    match ensure_task_conversation(&state.pg, &id).await {
        Ok(Some(conversation_id)) => {
            // The binder, for the client to render the thread's agent — the
            // conversations row is the one truth for it (rebind keeps it
            // tracking assignment), so read it back rather than recomputing
            // the ladder here.
            let binder: Option<(String,)> =
                sqlx::query_as("select agent_model from conversations where id = $1::uuid")
                    .bind(&conversation_id)
                    .fetch_optional(&state.pg)
                    .await
                    .ok()
                    .flatten();
            Json(json!({
                "conversationId": conversation_id,
                "agentModel": binder.map(|(m,)| m),
            }))
            .into_response()
        }
        // Nobody could own the thread — the ladders walked off the edge of
        // an empty board. Saying so beats a 500; adding a board member
        // fixes it.
        Ok(None) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "this ticket has no owner to hold its thread — add someone to its board" })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[tasks] conversation ensure failed: {e}");
            thrown_internal_error()
        }
    }
}
