// /api/research/{id}/conversation.
//
// OPEN THE CONVERSATION FOR A RUN, creating it the first time.
//
// ON DEMAND, and that is the whole reason this is a POST rather than a field
// that always exists. Most research runs are read once and never discussed; a
// conversation row per run would turn the chat list into a list of things
// nobody said anything about. The first person to open the thread is what
// makes it.
//
// ACCESS IS THE RUN'S ACCESS. `research_role` is the same check the report
// itself goes through — owner, member, or an org run with no owner — so
// nobody can talk in a room about a report they cannot read. Deliberately not
// a second permission: two answers to "may this person see this run" is how
// they drift.

use crate::error::{house_error, thrown_internal_error};
use crate::research::{ensure_research_conversation, research_role};
use crate::session::require_user;
use crate::state::AppState;
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
    if let Some(gate) = crate::params::uuid_gate("research", "POST conversation", &id) {
        return gate;
    }
    match research_role(&state.pg, Some(&user.id), &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[research] role read on conversation failed: {e}");
            return thrown_internal_error();
        }
    }
    match ensure_research_conversation(&state.pg, &id).await {
        Ok(Some(conversation_id)) => Json(json!({ "conversationId": conversation_id })).into_response(),
        // A run an AGENT started for the org has no human owner, so there is
        // nobody to own the conversation. The report is still readable; it
        // just cannot be talked in, and saying so beats a 500.
        Ok(None) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "this run has no owner, so it has no conversation — it was started by an agent for the org" })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[research] conversation ensure failed: {e}");
            thrown_internal_error()
        }
    }
}
