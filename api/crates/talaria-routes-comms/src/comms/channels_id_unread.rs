// /api/channels/{id}/unread.
// POST → mark the channel (or DM, or relay) unread for the caller: their
// channel cursor steps back to just before the newest message on the main
// feed that counts for them (complete, someone else's), so its badge reads 1
// again. → { ok, marked }; marked is false, and nothing is written, when the
// feed holds nothing that could count. The read route's twin on the same cursor. A
// member only (403 otherwise, whether or not the channel exists — the
// channels family's rule). A room whose roster is a board's or a team's has
// no cursor row of the caller's own to move: 409, as the notify level is.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_channels::{channel_role, mark_channel_unread};
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
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[channels] role read on unread failed", e)),
    }
    Ok(match mark_channel_unread(&state.pg, &id, &user.id).await {
        Ok(Some(marked)) => Json(json!({ "ok": true, "marked": marked })).into_response(),
        Ok(None) => house_error(
            StatusCode::CONFLICT,
            "you're in this room through its board or a team, so it has no read marker of your own to move",
        ),
        Err(e) => internal("[channels] mark unread failed", e),
    })
}
