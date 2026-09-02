// /api/boards/{id}/events — port of ui/src/routes/api/boards.$id.events.ts.
// SSE stream of this board's live events (task/comment changes), auth-gated
// to board members. Powers multiplayer boards. The stream itself is
// realtime's (board:<id> topic) and crossed with the publish plane; this
// route is only the gate in front of it.

use crate::boards::board_role;
use crate::error::{house_error, thrown_internal_error};
use crate::realtime::{RealtimeDeps, board_event_stream};
use crate::session::require_user;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = crate::params::uuid_gate("boards", "GET events", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards] role read on events failed: {e}");
            return thrown_internal_error();
        }
    }
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    board_event_stream(&deps, &id).await
}
