// /api/channels/{id}/events.
// SSE stream of the channel's live events (messages, membership),
// auth-gated to members. Powers multiplayer chat. The stream itself is
// realtime's (channel:<id> topic); this route is only the gate in front of it.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use talaria_channels::channel_role;
use talaria_error::{house_error, internal};
use talaria_realtime_watch::{RealtimeDeps, channel_event_stream};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[channels] role read on events failed", e)),
    }
    let deps = RealtimeDeps::streams_only(&state.cfg.redis_url);
    Ok(channel_event_stream(&deps, &id).await)
}
