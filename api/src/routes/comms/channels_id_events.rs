// /api/channels/{id}/events.
// SSE stream of the channel's live events (messages, membership),
// auth-gated to members. Powers multiplayer chat. The stream itself is
// realtime's (channel:<id> topic); this route is only the gate in front of it.

use crate::channels::channel_role;
use crate::error::{house_error, thrown_internal_error};
use crate::realtime::{RealtimeDeps, channel_event_stream};
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
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on events failed: {e}");
            return thrown_internal_error();
        }
    }
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    channel_event_stream(&deps, &id).await
}
