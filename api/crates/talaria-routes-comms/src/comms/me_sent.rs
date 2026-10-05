// GET /api/me/sent. The Comms sidebar's "Drafts & sent" view, sent half (drafts
// live in the browser): the viewer's own latest 50 messages, newest first —
// channel and DM messages in channels they are still in, and their turns in
// their agent DMs. Never anyone else's message. Answers
// { messages: [{ kind: 'channel' | 'agent', id, conversationId, title,
// content, createdAt }] }; `conversationId` is the channel id for 'channel'
// and the conversation id for 'agent'.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_channels::list_my_sent;
use talaria_error::internal;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_my_sent(&state.pg, &user.id).await {
        Ok(messages) => Json(json!({ "messages": messages })).into_response(),
        Err(e) => internal("[me/sent] read failed", e),
    })
}
