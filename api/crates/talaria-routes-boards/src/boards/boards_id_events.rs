// /api/boards/{id}/events. SSE stream of this board's live events
// (task/comment changes), auth-gated to board members. Powers multiplayer
// boards. The stream itself is realtime's (board:<id> topic, fed by the
// publish plane); this route is only the gate in front of it.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use talaria_boards::board_role;
use talaria_error::{house_error, internal};
use talaria_realtime_watch::{RealtimeDeps, board_event_stream};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET events", &id) {
        return gate;
    }
    match board_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => return internal("[boards] role read on events failed", e),
    }
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    board_event_stream(&deps, &id).await
}
