// /api/inbox/focus/conversations — port of
// ui/src/routes/api/inbox.focus.conversations.ts.
// GET → the panel's chat picker. POST → start a fresh conversation instance.
// Segmentation is the context strategy: a new instance is how old context is
// shed, and it is the owner's choice to make (no budget imposes it).

use crate::error::house_error;
use crate::inbox_focus_conversation::{create_inbox_conversation, list_inbox_conversations};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    match list_inbox_conversations(&state.pg, &user.id).await {
        Ok(conversations) => (
            StatusCode::OK,
            Json(json!({ "conversations": conversations })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[inbox-focus] conversation list failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };
    // TS passes `null` for the agent model — the instance starts model-less
    // and picks up the owner's assistant at command time.
    match create_inbox_conversation(&state.pg, &user.id, None).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(json!({ "conversation": { "id": id } })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("[inbox-focus] conversation create failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}
