// /api/home/queues. GET → `{ queues }`: the signed-in user's three human
// queues (triage, review, blocked), and nothing else.
//
// The web Inbox's board reads only the queues, every 30 seconds. /api/home
// answers them too, but for an admin it also computes the live alert count
// (a docker ps and the MCP, qdrant and embedding probes) and the cost
// overview, plus the activity feed, the org profile and two counts, and the
// board threw all of it away. This answers from the same `home_queues` pass
// /api/home uses (one statement), so the lanes come out identical.
// /api/home stays whole: it is the mobile app's landing read.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::internal;
use talaria_home::home_queues;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match home_queues(&state.pg, &user.id).await {
        Ok(queues) => Json(json!({ "queues": queues })).into_response(),
        Err(e) => internal("[home] queues failed", e),
    })
}

#[cfg(test)]
#[path = "home_queues_live.rs"]
mod home_queues_live;
