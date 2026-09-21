// /api/inbox/focus/conversations. GET → the panel's chat picker. POST →
// start a fresh conversation instance. Segmentation is the context
// strategy: a new instance is how old context is shed, and it is the
// owner's choice to make (no budget imposes it).

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_error::internal;
use talaria_inbox_focus::conversation::{create_inbox_conversation, list_inbox_conversations};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_inbox_conversations(&state.pg, &user.id).await {
        Ok(conversations) => (
            StatusCode::OK,
            Json(json!({ "conversations": conversations })),
        )
            .into_response(),
        Err(e) => internal("[inbox-focus] conversation list failed", e),
    })
}

pub async fn post(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(
        // no agent model at create — the instance starts model-less and picks
        // up the owner's assistant at command time.
        match create_inbox_conversation(&state.pg, &user.id, None).await {
            Ok(id) => (
                StatusCode::CREATED,
                Json(json!({ "conversation": { "id": id } })),
            )
                .into_response(),
            Err(e) => internal("[inbox-focus] conversation create failed", e),
        },
    )
}
