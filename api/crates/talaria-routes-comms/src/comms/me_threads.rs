// GET /api/me/threads. The Comms sidebar's Threads view: every thread root
// with at least one reply, in a channel the viewer is in right now, that the
// viewer started or replied in — newest reply first, at most 50. Answers
// { threads: [{ channelId, channelName, channelKind, root, lastAt }] }, where
// `root` is the channel message wire (with its reactions and reply rollup)
// and `channelName` is the other person's name for a DM.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_channels::list_my_threads;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_my_threads(&state.pg, &user.id).await {
        Ok(threads) => Json(json!({ "threads": threads })).into_response(),
        Err(e) => internal("[me/threads] read failed", e),
    })
}
