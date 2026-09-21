// GET /api/integrations/google/pending — the caller's agent-drafted actions
// awaiting their approval (send email / create event).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::google::pending_actions::{list_pending, pending_wire};
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let pending = match list_pending(&state.pg, &user.id, user.role == "admin").await {
        Ok(p) => p,
        Err(e) => return internal("[integrations/google/pending] list failed", e),
    };
    Json(json!({ "pending": pending.iter().map(pending_wire).collect::<Vec<_>>() })).into_response()
}
