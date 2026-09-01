// GET /api/integrations/google/pending — port of
// ui/src/routes/api/integrations/google.pending.ts. The caller's
// agent-drafted actions awaiting their approval (send email / create event).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::error::thrown_internal_error;
use crate::google_pending_actions::{list_pending, pending_wire};
use crate::session::require_user;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let pending = match list_pending(&state.pg, &user.id, user.role == "admin").await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[integrations/google/pending] list failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "pending": pending.iter().map(pending_wire).collect::<Vec<_>>() })).into_response()
}
